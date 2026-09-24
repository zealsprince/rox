//! The artist grid panel: the catalog's people as a wall of square tiles,
//! the album grid's shape one level up. Tiles show the artist's portrait
//! when the setting is on, otherwise their first album's cover.
//!
//! One tile per credited album artist, the library's grouping rule, so a
//! record's guests stay on the act that released it. A setting regroups on
//! the track artist; see [`ArtistGroup`].
//!
//! Clicking a tile writes the artist onto the shared filter, the field the
//! filter panel's Artist column writes, so every global-following panel
//! narrows to them. The wall leaves that field out of its own mask, so
//! picking never collapses the shelf you picked from. A double click plays
//! the artist instead.
//!
//! Deliberately not a library view mode: per the workspace rule, browsing
//! surfaces are panels of their own.

use std::collections::HashSet;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    Along, AnyElement, App, Axis, Context, Div, Entity, EventEmitter, FocusHandle, Focusable,
    Image, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, ObjectFit, Pixels,
    ScrollStrategy, ScrollWheelEvent, SharedString, Size, Subscription, WeakEntity, Window, canvas,
    div, img, prelude::*, px, size, svg,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::{Icon, Side, VirtualListScrollHandle, h_virtual_list, v_virtual_list};
use rox_core::QUEUE_CAP;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::TrackKey;
use rox_library::projection::{FilterField, FilterSet, Projection, SortKey, SymTable};
use rox_panel_api::actions::{TypeAheadNext, TypeAheadPrev};
use rox_panel_kit::config::{default_true, is_zero};
use rox_panel_kit::wall::{TILE_DIM_MAX, TILE_LABEL_H, WallLayout, default_dim, default_gap};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::grid::{LetterSide, TitleAlign};
use crate::panel::{
    self, AppState, FlickState, PanelChrome, PanelSettings, ResumeIdle, ScrubState, setting_row,
    toggle,
};
use crate::panel_settings;
use crate::query::search::{SearchBox, SearchEvent};
use crate::query::shared_query::{QueryFilter, QuerySource, SharedQueryEvent};
use crate::selection::SelectionEvent;
use crate::settings::ui as settings_ui;
use crate::thumbs::Thumb;

/// The tile size knob's range, in px. The top is the stored thumbnail's
/// long side, so scrubbing never upscales.
const TILE_MIN: f32 = 96.;
const TILE_MAX: f32 = 256.;

const TILE_ROUNDING_MAX: f32 = 100.;

const TILE_GAP_MAX: f32 = 24.;

const FALLBACK_COLS: usize = 5;

/// Rows of tiles asked for past each edge of the viewport, so a scroll
/// reveals loaded art instead of placeholders.
const PREFETCH_ROWS: usize = 2;

fn default_tile() -> f32 {
    160.
}

fn default_rounding() -> f32 {
    100.
}

/// The album artist folds guests onto the record they appear on. The track
/// artist gives every "feat." credit a tile of its own.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtistGroup {
    #[default]
    AlbumArtist,
    Artist,
}

impl ArtistGroup {
    fn label(self) -> SharedString {
        match self {
            ArtistGroup::AlbumArtist => rox_i18n::t!("filter-field-album-artist"),
            ArtistGroup::Artist => rox_i18n::t!("artist-grid-track-artist"),
        }
    }

    fn field(self) -> FilterField {
        match self {
            ArtistGroup::AlbumArtist => FilterField::AlbumArtist,
            ArtistGroup::Artist => FilterField::Artist,
        }
    }

    /// The column the runs break on and the table its symbols name.
    fn source(self, projection: &Projection) -> (&[u32], &SymTable) {
        match self {
            ArtistGroup::AlbumArtist => (&projection.album_artist, &projection.album_artists),
            ArtistGroup::Artist => (&projection.artist, &projection.artists),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ArtistGridConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub query: String,
    /// The query only applies while the search box shows.
    #[serde(default)]
    pub search: bool,
    #[serde(default)]
    pub query_source: QuerySource,
    #[serde(default)]
    pub group: ArtistGroup,
    #[serde(default = "default_true")]
    pub vertical: bool,
    #[serde(default)]
    pub letters: bool,
    #[serde(default)]
    pub letters_compact: bool,
    #[serde(default)]
    pub letters_side: LetterSide,
    /// Preferred tile edge in px. A typed size can go past [`TILE_MAX`].
    #[serde(default = "default_tile")]
    pub tile: f32,
    /// Picking writes the shared filter. Off leaves the pick as a plain
    /// selection.
    #[serde(default = "default_true")]
    pub pick_filters: bool,
    /// Off by default, so a big wall doesn't reach for the network unasked.
    #[serde(default)]
    pub portraits: bool,
    #[serde(default)]
    pub follow_playing: bool,
    /// Slide back to the playing artist after the wall sits idle.
    #[serde(default)]
    pub resume_playing: bool,
    #[serde(default)]
    pub smooth_follow: bool,
    #[serde(default)]
    pub dim_playing: bool,
    #[serde(default)]
    pub desaturate_playing: bool,
    /// Dim and desaturate even when nothing plays.
    #[serde(default)]
    pub dim_always: bool,
    /// How far the dimmed tiles fade, in percent of fully hidden.
    #[serde(default = "default_dim")]
    pub dim: f32,
    /// Corner rounding, in percent of circular.
    #[serde(default = "default_rounding")]
    pub rounding: f32,
    #[serde(default = "default_gap")]
    pub gap: f32,
    #[serde(default = "default_true")]
    pub labels: bool,
    #[serde(default)]
    pub label_align: TitleAlign,
    #[serde(default = "default_true")]
    pub counts: bool,
    /// The top-left cell index at save time. A cell index survives a tile
    /// size or width change.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub scroll: usize,
}

impl Default for ArtistGridConfig {
    fn default() -> Self {
        ArtistGridConfig {
            chrome: PanelChrome::default(),
            query: String::new(),
            search: false,
            query_source: QuerySource::default(),
            group: ArtistGroup::default(),
            vertical: true,
            letters: false,
            letters_compact: false,
            letters_side: LetterSide::default(),
            tile: default_tile(),
            pick_filters: true,
            portraits: false,
            follow_playing: false,
            resume_playing: false,
            smooth_follow: false,
            dim_playing: false,
            desaturate_playing: false,
            dim_always: false,
            dim: default_dim(),
            rounding: default_rounding(),
            gap: default_gap(),
            labels: true,
            label_align: TitleAlign::default(),
            counts: true,
            scroll: 0,
        }
    }
}

/// One artist's run in the current view. `art` resolves on first paint; the
/// inner None is an artist with nothing to show.
struct Cell {
    /// The symbol in whichever table the wall groups by.
    sym: u32,
    start: usize,
    len: u32,
    albums: u32,
    art: Option<Option<PathBuf>>,
    /// Eased opacity under the dim mode. None until first paint, which starts
    /// at the target.
    dim: Option<f32>,
    /// The portrait's crossfade over the cover, 0 to 1. None until first
    /// paint, so a face already in hand arrives solid on a rebuild.
    face: Option<f32>,
    /// Learned on paint, so the fade loop doesn't re-probe the cache by name
    /// every frame.
    faced: bool,
}

pub struct ArtistGridPanel {
    state: AppState,
    config: ArtistGridConfig,
    /// The canonical order while nothing narrows it, otherwise the hits
    /// re-sorted canonically so an artist stays one contiguous run.
    view: Arc<Vec<u32>>,
    cells: Vec<Cell>,
    letters: Vec<(SharedString, usize)>,
    /// A rail click's letter, pinned until a real scroll or another jump
    /// hands the highlight back to the first visible tile.
    letter_hold: Option<usize>,
    search: Entity<SearchBox>,
    /// While `pick_filters` is on this mirrors the shared filter, so a chip
    /// cleared elsewhere lifts the outline here too.
    selected: HashSet<usize>,
    /// Where a shift-extend grows from: the last plain or toggle click.
    anchor: Option<usize>,
    /// A click sets it too, so the keyboard carries on from the pointer.
    cursor: Option<usize>,
    hovered: Option<usize>,
    /// The width while scrolling vertically, the height otherwise. The dock
    /// caches panels, so the list closure notifies when the painted extent
    /// drifts from this.
    cross: Pixels,
    scroll: VirtualListScrollHandle,
    flick: FlickState,
    glide_to: Option<usize>,
    /// The saved top-left cell, held until the wall has artists and a
    /// measured width. A user drag clears it.
    restore: Option<usize>,
    last_tick: Instant,
    resume_idle: ResumeIdle,
    playing_key: Option<TrackKey>,
    /// Kept fresh so per-frame dimming never rescans.
    playing_ix: Option<usize>,
    playing: bool,
    dim_fading: bool,
    /// Armed by the tile that first sees a face it hasn't faded in yet.
    face_fading: bool,
    tile_scrub: ScrubState,
    rounding_scrub: ScrubState,
    gap_scrub: ScrubState,
    dim_scrub: ScrubState,
    value_edit: panel::ValueEdit,
    error: Option<SharedString>,
    /// Applied on the next render, where a window exists to set the input.
    resync_box: bool,
    /// Runtime only: a restore re-pins from whatever is picked then.
    selection_ids: Vec<i64>,
    type_ahead: String,
    type_ahead_at: Option<Instant>,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _library_changed: Subscription,
    _thumbs_changed: Subscription,
    _portraits_changed: Subscription,
    _search_events: Subscription,
    _query_changed: Subscription,
    _selection_changed: Subscription,
    _player_changed: Subscription,
    /// Drops the phrase on blur so tab goes back to walking panels.
    _type_ahead_blur: Subscription,
}

impl ArtistGridPanel {
    pub fn new(
        state: AppState,
        config: ArtistGridConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.rebuild(cx);
                // The catalog loads after a restored track starts, so the
                // launch's follow waits for this first rebuild.
                if this.config.follow_playing {
                    this.follow_playing(cx);
                }
            },
        );
        let _thumbs_changed = cx.observe(&state.thumbs, |_, _, cx| cx.notify());
        let _portraits_changed = cx.observe(&state.portraits, |_, _, cx| cx.notify());
        let initial = match config.query_source {
            QuerySource::Global => state.query.read(cx).text().to_string(),
            QuerySource::Local | QuerySource::Selection => config.query.clone(),
        };
        let search =
            cx.new(|cx| SearchBox::new(rox_i18n::t!("query-search"), &initial, window, cx).small());
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        // Filter changes arrive here too, including our own picks, which
        // keeps the outlines honest when another surface clears them.
        let _query_changed = cx.subscribe(
            &state.query,
            |this: &mut Self, _, _: &SharedQueryEvent, cx| {
                this.on_shared_query_changed(cx);
            },
        );
        let selection_ids = state.selection.read(cx).tracks().to_vec();
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, event: &SelectionEvent, cx| {
                this.on_selection_changed(event.source, cx);
            },
        );
        let _player_changed = cx.observe(&state.player, |this: &mut Self, _, cx| {
            this.sync_playing(cx)
        });
        // Follow-playing owns the position on launch, so it skips the saved
        // scroll.
        let restore = (!config.follow_playing && config.scroll > 0).then_some(config.scroll);
        let focus = cx.focus_handle().tab_stop(true);
        let panel = cx.weak_entity();
        let _type_ahead_blur = window.on_focus_out(&focus, cx, move |_, _, cx| {
            panel
                .update(cx, |this: &mut ArtistGridPanel, cx| {
                    this.clear_type_ahead(cx);
                })
                .ok();
        });
        let mut this = ArtistGridPanel {
            state,
            config,
            view: Arc::new(Vec::new()),
            cells: Vec::new(),
            letters: Vec::new(),
            letter_hold: None,
            search,
            selected: HashSet::new(),
            anchor: None,
            cursor: None,
            hovered: None,
            cross: px(0.),
            scroll: VirtualListScrollHandle::new(),
            flick: FlickState::default(),
            glide_to: None,
            restore,
            last_tick: Instant::now(),
            resume_idle: ResumeIdle::default(),
            playing_key: None,
            playing_ix: None,
            playing: false,
            dim_fading: false,
            face_fading: false,
            tile_scrub: ScrubState::default(),
            rounding_scrub: ScrubState::default(),
            gap_scrub: ScrubState::default(),
            dim_scrub: ScrubState::default(),
            value_edit: panel::ValueEdit::default(),
            error: None,
            resync_box: false,
            selection_ids,
            type_ahead: String::new(),
            type_ahead_at: None,
            focus,
            tab_panel: None,
            _library_changed,
            _thumbs_changed,
            _portraits_changed,
            _search_events,
            _query_changed,
            _selection_changed,
            _player_changed,
            _type_ahead_blur,
        };
        this.rebuild(cx);
        // A duplicate opens with a track already playing.
        this.sync_playing(cx);
        this
    }

    /// The player notifies every pump, so the compares keep this cheap.
    fn sync_playing(&mut self, cx: &mut Context<Self>) {
        let (playing, path) = {
            let player = self.state.player.read(cx);
            (player.is_playing(), player.now_playing().map(|now| now.key))
        };
        if playing != self.playing {
            self.playing = playing;
            self.dim_fading = true;
            cx.notify();
        }
        if path == self.playing_key {
            return;
        }
        self.playing_key = path;
        self.playing_ix = self.playing_cell(cx);
        self.dim_fading = true;
        if self.config.follow_playing {
            self.follow_playing(cx);
        }
        cx.notify();
    }

    fn playing_cell(&self, cx: &App) -> Option<usize> {
        let key = self.playing_key.as_ref()?;
        let library = self.state.library.read(cx);
        let id = library.id_for_key(key)?;
        let projection = library.projection()?;
        let view_ix = self
            .view
            .iter()
            .position(|&row| projection.db_id[row as usize] == id)?;
        Some(
            self.cells
                .partition_point(|cell| cell.start <= view_ix)
                .saturating_sub(1),
        )
    }

    fn follow_playing(&mut self, cx: &mut Context<Self>) {
        let Some(cell_ix) = self.playing_ix else {
            return;
        };
        self.letter_hold = None;
        // Both modes head for a line, not an offset: the line is stable
        // while the layout may still be settling.
        self.glide_to = Some(cell_ix / self.lanes());
        cx.notify();
    }

    /// The automatic follow never touches the picks; the menu's jump does.
    fn jump_to_playing(&mut self, cx: &mut Context<Self>) {
        let Some(cell_ix) = self.playing_ix else {
            return;
        };
        self.selected = HashSet::from([cell_ix]);
        self.anchor = Some(cell_ix);
        self.cursor = Some(cell_ix);
        self.publish(cx);
        self.follow_playing(cx);
    }

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

    fn toggle_follow_playing(&mut self, cx: &mut Context<Self>) {
        self.config.follow_playing = !self.config.follow_playing;
        if self.config.follow_playing {
            self.follow_playing(cx);
        }
        cx.notify();
    }

    /// Lanes and tile edge key off the cross extent, so drop it and let the
    /// next paint re-measure.
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

    /// The shared filter minus the wall's own field, or a pick would collapse
    /// the wall to the one tile you clicked.
    fn browse_filter(&self, cx: &App) -> FilterSet {
        let mut filter = self.effective_filter(cx);
        filter.clear(self.config.group.field());
        filter
    }

    /// The picks name values in the field the wall is leaving, so they go
    /// with it.
    fn set_group(&mut self, group: ArtistGroup, cx: &mut Context<Self>) {
        if self.config.group == group {
            return;
        }
        self.drop_artist_filter(cx);
        self.config.group = group;
        self.rebuild(cx);
    }

    /// Hits filter an ordered view rather than being iterated, or an
    /// artist's scattered rows would split into duplicate tiles. The
    /// canonical order already groups by album artist; a track-artist wall
    /// re-sorts, since a guest's rows sit a shelf apart.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.cells.clear();
        self.selected.clear();
        self.view = {
            let query = self.effective_query(cx);
            let filter = self.browse_filter(cx);
            let library = self.state.library.read(cx);
            match library.projection() {
                Some(projection) => {
                    let mask = projection.filter_mask(&filter);
                    let rows = if query.is_empty() && mask.is_none() {
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
                    };
                    match self.config.group {
                        ArtistGroup::AlbumArtist => rows,
                        // Ties keep the canonical order, so a run still
                        // reads album by album.
                        ArtistGroup::Artist => {
                            Arc::new(projection.sort_view(&rows, SortKey::Artist, false))
                        }
                    }
                }
                None => Arc::new(Vec::new()),
            }
        };
        if let Some(projection) = self.state.library.read(cx).projection() {
            let (column, _) = self.config.group.source(projection);
            let mut last_album = None;
            for (i, &row) in self.view.iter().enumerate() {
                let sym = column[row as usize];
                let album = projection.album[row as usize];
                if self.cells.last().map(|cell| cell.sym) != Some(sym) {
                    self.cells.push(Cell {
                        sym,
                        start: i,
                        len: 0,
                        albums: 0,
                        art: None,
                        dim: None,
                        face: None,
                        faced: false,
                    });
                    last_album = None;
                }
                let cell = self.cells.last_mut().unwrap();
                cell.len += 1;
                // Albums sort within artist, so a symbol change is a new
                // record.
                if last_album != Some(album) {
                    cell.albums += 1;
                    last_album = Some(album);
                }
            }
        }
        // A pick comes back around as this rebuild, so keep the anchor and
        // hover (clamped) or a click breaks shift-extend.
        self.anchor = self.anchor.filter(|&ix| ix < self.cells.len());
        self.cursor = self.cursor.filter(|&ix| ix < self.cells.len());
        self.hovered = self.hovered.filter(|&ix| ix < self.cells.len());
        self.letters.clear();
        if let Some(projection) = self.state.library.read(cx).projection() {
            let (_, table) = self.config.group.source(projection);
            for (ix, cell) in self.cells.iter().enumerate() {
                // The ordering's key, so the rail stays monotonic: a
                // sort-tagged 米津玄師 sorts under Y and the rail says Y.
                let name = table.sort_key(cell.sym as usize);
                let letter = panel::letter_initial(name);
                if self.letters.last().map(|(l, _)| l.as_ref()) != Some(letter.as_str()) {
                    self.letters.push((SharedString::from(letter), ix));
                }
            }
        }
        self.sync_picks(cx);
        self.playing_ix = self.playing_cell(cx);
        cx.notify();
    }

    fn letter_rail(&self, cx: &mut Context<Self>) -> Option<Div> {
        if !self.config.letters {
            return None;
        }
        let first = self.letter_hold.unwrap_or_else(|| self.first_cell());
        let active = self
            .letters
            .iter()
            .rposition(|&(_, ix)| ix <= first)
            .unwrap_or(0);
        let horizontal = self.axis() == Axis::Horizontal;
        let start = self.config.letters_side == LetterSide::Start;
        let rail = panel::letter_rail(
            &self.letters,
            active,
            horizontal,
            self.config.letters_compact,
            |this: &mut Self, first, cx| {
                this.touch_resume(cx);
                this.scroll_to_letter(first, cx);
            },
            cx,
        )?;
        Some(if horizontal {
            div()
                .flex_none()
                .w_full()
                .py(px(2.))
                .map(|d| {
                    if start {
                        d.border_b_1()
                    } else {
                        d.border_t_1()
                    }
                })
                .border_color(palette::border())
                .child(rail)
        } else {
            div()
                .flex_none()
                .h_full()
                .px(px(2.))
                .map(|d| {
                    if start {
                        d.border_r_1()
                    } else {
                        d.border_l_1()
                    }
                })
                .border_color(palette::border())
                .child(rail)
        })
    }

    /// While the wall drives the filter, the filter is the one source of
    /// truth for the outlines.
    fn sync_picks(&mut self, cx: &App) {
        if !self.config.pick_filters {
            return;
        }
        let picks = self
            .state
            .query
            .read(cx)
            .filter()
            .values(self.config.group.field())
            .to_vec();
        if picks.is_empty() {
            return;
        }
        let library = self.state.library.read(cx);
        let Some(projection) = library.projection() else {
            return;
        };
        let (_, table) = self.config.group.source(projection);
        self.selected = self
            .cells
            .iter()
            .enumerate()
            .filter(|(_, cell)| {
                let name = &table.strings[cell.sym as usize];
                picks.iter().any(|pick| pick == name)
            })
            .map(|(ix, _)| ix)
            .collect();
    }

    /// The title row only repaints when the tab panel is notified.
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

    fn cell_name(&self, ix: usize, cx: &App) -> String {
        let Some(cell) = self.cells.get(ix) else {
            return String::new();
        };
        let library = self.state.library.read(cx);
        match library.projection() {
            Some(projection) => {
                self.config.group.source(projection).1.strings[cell.sym as usize].clone()
            }
            None => String::new(),
        }
    }

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

    /// The first track with an album tag, so a loose track's art never
    /// stands in for a whole shelf.
    fn art_path(&mut self, ix: usize, cx: &Context<Self>) -> Option<PathBuf> {
        if let Some(art) = self.cells.get(ix).and_then(|cell| cell.art.clone()) {
            return art;
        }
        let path = {
            let library = self.state.library.read(cx);
            let id = self.cells.get(ix).and_then(|cell| {
                let projection = library.projection()?;
                let row = self
                    .view
                    .get(cell.start..cell.start + cell.len as usize)?
                    .iter()
                    .copied()
                    .find(|&row| !projection.resolve(row).album.is_empty())?;
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

    /// None is a face still on its way or one no service knows. The tile
    /// shows an album cover either way.
    fn portrait(&mut self, ix: usize, cx: &mut Context<Self>) -> Option<Arc<Image>> {
        let name = self.cell_name(ix, cx);
        let portraits = self.state.portraits.clone();
        portraits.update(cx, |portraits, cx| portraits.get(&name, cx))
    }

    /// The library's click rules, by tile.
    fn select(&mut self, ix: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        self.cursor = Some(ix);
        if modifiers.shift {
            let anchor = self.anchor.unwrap_or(ix);
            let (lo, hi) = (anchor.min(ix), anchor.max(ix));
            // Ctrl+Shift stacks the range onto the picks; plain shift
            // replaces.
            if modifiers.secondary() {
                self.selected.extend(lo..=hi);
            } else {
                self.selected = (lo..=hi).collect();
            }
            if self.anchor.is_none() {
                self.anchor = Some(anchor);
            }
        } else if modifiers.secondary() {
            if !self.selected.insert(ix) {
                self.selected.remove(&ix);
            }
            self.anchor = Some(ix);
        } else {
            // A second plain click on the lone pick drops it, so the wall is
            // its own way back to the whole catalog.
            if self.selected.len() == 1 && self.selected.contains(&ix) {
                self.selected.clear();
            } else {
                self.selected = HashSet::from([ix]);
            }
            self.anchor = Some(ix);
        }
        self.publish(cx);
        cx.notify();
    }

    fn set_cursor(&mut self, ix: usize, extend: bool, cx: &mut Context<Self>) {
        if ix >= self.cells.len() {
            return;
        }
        self.cursor = Some(ix);
        if extend {
            let anchor = self.anchor.unwrap_or(ix);
            let (lo, hi) = (anchor.min(ix), anchor.max(ix));
            self.selected = (lo..=hi).collect();
            self.anchor = Some(anchor);
        } else {
            self.selected = HashSet::from([ix]);
            self.anchor = Some(ix);
        }
        self.publish(cx);
        self.scroll_to_cell(ix, cx);
    }

    /// With no cursor, the first press lands on the edge the step heads
    /// toward.
    fn move_cursor(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let len = self.cells.len();
        if len == 0 {
            return;
        }
        let target = match self.cursor {
            None if delta >= 0 => 0,
            None => len - 1,
            Some(cursor) => (cursor as isize + delta).clamp(0, len as isize - 1) as usize,
        };
        self.set_cursor(target, extend, cx);
    }

    fn publish(&mut self, cx: &mut Context<Self>) {
        self.publish_selection(cx);
        self.publish_picks(cx);
    }

    fn publish_selection(&mut self, cx: &mut Context<Self>) {
        let mut ixs: Vec<usize> = self.selected.iter().copied().collect();
        ixs.sort_unstable();
        let ids: Vec<i64> = ixs
            .iter()
            .flat_map(|&ix| self.ids_for(ix, cx))
            .take(QUEUE_CAP)
            .collect();
        let source = cx.entity_id();
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, source, cx));
    }

    fn publish_picks(&mut self, cx: &mut Context<Self>) {
        if !self.config.pick_filters {
            return;
        }
        let mut ixs: Vec<usize> = self.selected.iter().copied().collect();
        ixs.sort_unstable();
        let names: Vec<String> = ixs.iter().map(|&ix| self.cell_name(ix, cx)).collect();
        let field = self.config.group.field();
        self.state.query.clone().update(cx, |query, cx| {
            let mut filter = query.filter().clone();
            filter.clear(field);
            for name in names {
                filter.toggle(field, &name);
            }
            query.set_filter(filter, cx);
        });
    }

    fn clear_picks(&mut self, cx: &mut Context<Self>) {
        self.selected.clear();
        self.anchor = None;
        self.cursor = None;
        self.publish_selection(cx);
        self.drop_artist_filter(cx);
        cx.notify();
    }

    /// Unconditional, unlike [`Self::publish_picks`]: turning picking off
    /// has to lift a filter the wall can no longer reach.
    fn drop_artist_filter(&mut self, cx: &mut Context<Self>) {
        let field = self.config.group.field();
        self.state.query.clone().update(cx, |query, cx| {
            let mut filter = query.filter().clone();
            filter.clear(field);
            query.set_filter(filter, cx);
        });
    }

    /// Modifiers pass through so the workspace keeps its shortcuts, and a
    /// leading space stays its play/pause.
    fn on_panel_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.alt {
            return;
        }
        // No select-all: it would pour every name into the shared filter.
        self.touch_resume(cx);
        let shift = keystroke.modifiers.shift;
        let key = keystroke.key.as_str();
        if let Some(delta) = self.wall().step(key) {
            self.move_cursor(delta, shift, cx);
            return;
        }
        match key {
            // A phrase drops first, since it's holding tab, then the picks.
            "escape" => {
                if !self.clear_type_ahead(cx) {
                    self.deselect(cx);
                }
            }
            "pageup" => self.move_cursor(-self.wall().page_step(), shift, cx),
            "pagedown" => self.move_cursor(self.wall().page_step(), shift, cx),
            "home" => self.set_cursor(0, shift, cx),
            "end" => {
                let last = self.cells.len().saturating_sub(1);
                self.set_cursor(last, shift, cx);
            }
            "enter" => self.play_cursor(cx),
            _ => {
                let Some(text) = &keystroke.key_char else {
                    return;
                };
                if text == " " && !panel::type_ahead_live(self.type_ahead_at) {
                    return;
                }
                // Stop here so space doesn't also fire the workspace's
                // TogglePlayback binding.
                cx.stop_propagation();
                self.type_to(text.clone(), cx);
            }
        }
    }

    fn play_cursor(&mut self, cx: &mut Context<Self>) {
        let mut ixs: Vec<usize> = self.selected.iter().copied().collect();
        ixs.sort_unstable();
        if ixs.len() > 1 {
            self.play_many(ixs, cx);
        } else if let Some(ix) = self.cursor.or_else(|| ixs.first().copied()) {
            self.play(ix, cx);
        }
    }

    fn deselect(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            return;
        }
        self.selected.clear();
        self.anchor = None;
        self.cursor = None;
        self.publish(cx);
        cx.notify();
    }

    /// A fresh phrase starts past the current pick, so the same letter steps
    /// to the next match. A grown one re-tests it so refining stays put.
    fn type_to(&mut self, text: String, cx: &mut Context<Self>) {
        let grown = panel::type_ahead_grow(&mut self.type_ahead, &mut self.type_ahead_at, text);
        // Repaint even on a miss: the badge changed.
        panel::type_ahead_fade(cx);
        cx.notify();
        let len = self.cells.len();
        if len == 0 {
            return;
        }
        let needle = self.type_ahead.to_lowercase();
        let anchor = self.selected.iter().copied().min().or(self.anchor);
        let start = match anchor {
            Some(ix) if grown => ix,
            Some(ix) => ix + 1,
            None => 0,
        };
        let hit = {
            let library = self.state.library.read(cx);
            library.projection().and_then(|projection| {
                let (_, table) = self.config.group.source(projection);
                (0..len).map(|off| (start + off) % len).find(|&ix| {
                    self.cells.get(ix).is_some_and(|cell| {
                        panel::type_ahead_hit(&table.lower[cell.sym as usize], &needle)
                    })
                })
            })
        };
        if let Some(ix) = hit {
            self.selected = HashSet::from([ix]);
            self.anchor = Some(ix);
            self.cursor = Some(ix);
            self.publish(cx);
            self.scroll_to_cell(ix, cx);
        }
    }

    /// True when there was a phrase, for the escape ladder.
    fn clear_type_ahead(&mut self, cx: &mut Context<Self>) -> bool {
        if self.type_ahead.is_empty() {
            return false;
        }
        self.type_ahead.clear();
        self.type_ahead_at = None;
        cx.notify();
        true
    }

    /// Tab's cycle. Leaves the window stamp alone so a run of tabs doesn't
    /// revive the badge.
    fn type_step(&mut self, back: bool, cx: &mut Context<Self>) {
        if self.type_ahead.is_empty() {
            return;
        }
        let len = self.cells.len();
        if len == 0 {
            return;
        }
        cx.notify();
        let needle = self.type_ahead.to_lowercase();
        let anchor = self.selected.iter().copied().min().or(self.anchor);
        let hit = {
            let library = self.state.library.read(cx);
            library.projection().and_then(|projection| {
                let (_, table) = self.config.group.source(projection);
                panel::type_ahead_scan(len, anchor, back).find(|&ix| {
                    self.cells.get(ix).is_some_and(|cell| {
                        panel::type_ahead_hit(&table.lower[cell.sym as usize], &needle)
                    })
                })
            })
        };
        if let Some(ix) = hit {
            self.selected = HashSet::from([ix]);
            self.anchor = Some(ix);
            self.cursor = Some(ix);
            self.publish(cx);
            self.scroll_to_cell(ix, cx);
        }
    }

    fn scroll_to_cell(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.letter_hold = None;
        self.scroll_to_cell_with(ix, ScrollStrategy::Center, cx);
    }

    fn scroll_to_letter(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.letter_hold = Some(ix);
        self.scroll_to_cell_with(ix, ScrollStrategy::Top, cx);
    }

    fn scroll_to_cell_with(&mut self, ix: usize, strategy: ScrollStrategy, cx: &mut Context<Self>) {
        self.glide_to = None;
        self.restore = None;
        let line = ix / self.lanes();
        self.scroll.scroll_to_item(line, strategy);
        cx.notify();
    }

    fn play(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.play_many(vec![ix], cx);
    }

    /// Plays as context, not queued entries, so the queue keeps what was
    /// hand-picked (ADR 16).
    fn play_many(&mut self, ixs: Vec<usize>, cx: &mut Context<Self>) {
        let ids: Vec<i64> = ixs
            .iter()
            .flat_map(|&ix| self.ids_for(ix, cx))
            .take(QUEUE_CAP)
            .collect();
        let result = self.state.library.read(cx).keys_for(&ids);
        match result {
            Ok(keys) => {
                self.error = None;
                self.state
                    .player
                    .update(cx, |player, cx| player.play(keys, cx));
            }
            Err(e) => {
                self.error = Some(format!("library: {e}").into());
                cx.notify();
            }
        }
    }

    fn wall(&self) -> WallLayout {
        WallLayout {
            cross: self.cross,
            tile: self.config.tile,
            gap: self.config.gap,
            labels: self.config.labels,
            vertical: self.config.vertical,
            dim: self.config.dim,
            dim_playing: self.config.dim_playing,
            dim_always: self.config.dim_always,
            desaturate_playing: self.config.desaturate_playing,
            hovered: self.hovered,
            playing_ix: self.playing_ix,
            playing: self.playing,
            fallback_lanes: FALLBACK_COLS,
            label_h: None,
        }
    }

    fn lanes(&self) -> usize {
        self.wall().lanes()
    }

    fn label_height(&self) -> f32 {
        self.wall().label_height()
    }

    fn axis(&self) -> Axis {
        self.wall().axis()
    }

    fn first_cell(&self) -> usize {
        self.wall().first_cell(
            self.restore,
            self.scroll.base_handle().offset(),
            self.cells.len(),
        )
    }

    fn tile_side(&self) -> Pixels {
        self.wall().tile_side()
    }

    fn dim_target(&self, ix: usize) -> f32 {
        self.wall().dim_target(ix)
    }

    fn desaturated(&self, ix: usize) -> bool {
        self.wall().desaturated(ix)
    }

    /// Pending and missing art share the placeholder, so an arriving face
    /// fills the tile without a flash.
    fn tile(&mut self, ix: usize, side: Pixels, cx: &mut Context<Self>) -> AnyElement {
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
        let face = self
            .config
            .portraits
            .then(|| self.portrait(ix, cx))
            .flatten();
        // A face already in hand seeds at 1, so only a late arrival fades.
        let faced = face.is_some();
        let target = if faced { 1. } else { 0. };
        let faded = match self.cells.get(ix).and_then(|cell| cell.face) {
            Some(faded) => faded,
            None => {
                if let Some(cell) = self.cells.get_mut(ix) {
                    cell.face = Some(target);
                }
                target
            }
        };
        // Wake the loop when progress and target disagree. The ease snaps
        // onto its target, so the compare can be exact.
        if let Some(cell) = self.cells.get_mut(ix) {
            cell.faced = faced;
        }
        if faded != target && !self.face_fading {
            self.face_fading = true;
            cx.notify();
        }
        // Dropped once the portrait covers it, so a settled wall of faces
        // stops touching the thumbnail cache.
        let show_cover = !faced || faded < 1.;
        let cover = show_cover
            .then(|| match self.art_path(ix, cx) {
                Some(path) => self
                    .state
                    .thumbs
                    .update(cx, |thumbs, cx| thumbs.get(&path, cx)),
                None => Thumb::Missing,
            })
            .and_then(|thumb| match thumb {
                Thumb::Ready(image) => Some(image),
                _ => None,
            });
        // Round the image itself too: gpui content masks stay rectangular, so
        // a square image would paint over the tile's rounded corners.
        let radius = side * (self.config.rounding / 200.);
        let desaturated = self.desaturated(ix);
        let layer = |image: Arc<Image>| {
            img(image)
                .size_full()
                .overflow_hidden()
                .object_fit(ObjectFit::Cover)
                .grayscale(desaturated)
                .rounded(radius)
        };
        let mut content = div().size_full().relative();
        if show_cover {
            content = match cover {
                Some(image) => content.child(layer(image)),
                None => content.child(
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            svg()
                                .path(icons::USER)
                                .size(px(24.))
                                .text_color(palette::text_faint()),
                        ),
                ),
            };
        }
        if let Some(image) = face {
            content = content.child(
                div()
                    .absolute()
                    .inset_0()
                    .when(faded < 1., |d| d.opacity(faded))
                    .child(layer(image)),
            );
        }
        let content = content.into_any_element();
        let labels = self.config.labels;
        let picked = self.selected.contains(&ix);
        let face_square = div()
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
            .when(picked, |d| {
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
                    this.dim_fading = true;
                    cx.notify();
                }
            }))
            // On release: a press might start a drag-scroll.
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
            .child(face_square)
            .when(labels, |d| d.child(self.caption(ix, side, picked, cx)))
            .into_any_element()
    }

    fn cell_labels(&self, ix: usize, cx: &App) -> (SharedString, SharedString, SharedString) {
        let Some(cell) = self.cells.get(ix) else {
            return Default::default();
        };
        let name = self.cell_name(ix, cx);
        // An untagged shelf gets no reading: its Unknown label is the app's
        // word, not a name anyone wrote.
        let reading = if name.is_empty() {
            String::new()
        } else {
            self.state
                .library
                .read(cx)
                .projection()
                .map(|projection| {
                    self.config
                        .group
                        .source(projection)
                        .1
                        .sort_name(cell.sym as usize)
                        .to_string()
                })
                .unwrap_or_default()
        };
        // Display only: the pick it writes stays the real empty string.
        let name = if name.is_empty() {
            rox_i18n::t!("filter-unknown").to_string()
        } else {
            name
        };
        let tally = if self.config.counts {
            rox_i18n::t!(
                "artist-grid-tally",
                albums = cell.albums as u64,
                tracks = cell.len as u64
            )
            .to_string()
        } else {
            String::new()
        };
        (
            SharedString::from(name),
            SharedString::from(reading),
            SharedString::from(tally),
        )
    }

    fn label(&self, ix: usize, cx: &App) -> Div {
        let (name, reading, tally) = self.cell_labels(ix, cx);
        let readings = crate::settings::show_readings();
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
            .child(
                div()
                    .truncate()
                    .text_color(palette::text_bright())
                    .child(panel::named(&name, &reading, readings)),
            )
            .when(!tally.is_empty(), |d| {
                d.child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(palette::text_secondary())
                        .child(tally),
                )
            })
    }

    /// A fixed-height block, so the virtual list's line pitch stays
    /// predictable.
    fn caption(&self, ix: usize, side: Pixels, picked: bool, cx: &App) -> Div {
        let (name, reading, tally) = self.cell_labels(ix, cx);
        let readings = crate::settings::show_readings();
        let base = div()
            .w(side)
            .h(px(TILE_LABEL_H))
            .pt(tokens::SPACE_XS)
            .flex()
            .flex_col()
            .overflow_hidden();
        match self.config.label_align {
            TitleAlign::Left => base.text_left(),
            TitleAlign::Center => base.text_center(),
            TitleAlign::Right => base.text_right(),
        }
        .child(
            div()
                .truncate()
                .text_sm()
                .text_color(if picked {
                    palette::accent()
                } else {
                    palette::text_bright()
                })
                .child(panel::named(&name, &reading, readings)),
        )
        .when(!tally.is_empty(), |d| {
            d.child(
                div()
                    .truncate()
                    .text_xs()
                    .text_color(palette::text_secondary())
                    .child(tally),
            )
        })
    }

    /// Solo or popped out there's no title bar to host the search.
    fn toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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

    /// Also where the painted extent reconciles, since a cached panel's
    /// resize repaints this closure without re-running render.
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
        // Warm the covers past both edges, after the visible tiles so those
        // keep first place in the load pool. Portraits only load for what
        // shows: their fetches are somebody else's bandwidth.
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

impl PanelSettings for ArtistGridPanel {
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
        &[("Layout", icons::ALIGN_LEFT)]
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
                rox_i18n::t!("grid-vertical-layout"),
                Some(rox_i18n::t!("grid-vertical-layout.description")),
                toggle(
                    self.config.vertical,
                    |this: &mut Self, on, cx| {
                        this.set_orientation(on, cx);
                    },
                    cx,
                ),
            ))
            .into_any_element()
    }

    fn behavior(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        Some(
            div()
                .flex()
                .flex_col()
                .gap(settings_ui::SECTION_GAP)
                .child(settings_ui::section(
                    rox_i18n::t!("artist-grid-section-grouping"),
                    None,
                    setting_row(
                        rox_i18n::t!("artist-grid-group-mode"),
                        Some(rox_i18n::t!("artist-grid-group-mode.description")),
                        panel::choices_shared(
                            &[
                                (
                                    rox_i18n::t!("filter-field-album-artist"),
                                    ArtistGroup::AlbumArtist,
                                ),
                                (
                                    rox_i18n::t!("artist-grid-track-artist"),
                                    ArtistGroup::Artist,
                                ),
                            ],
                            self.config.group,
                            |this: &mut Self, group, cx| this.set_group(group, cx),
                            cx,
                        ),
                    ),
                ))
                .child(settings_ui::section(
                    rox_i18n::t!("wall-section-picking"),
                    None,
                    setting_row(
                        rox_i18n::t!("artist-grid-pick-filters"),
                        Some(rox_i18n::t!("artist-grid-pick-filters.description")),
                        toggle(
                            self.config.pick_filters,
                            |this: &mut Self, on, cx| {
                                this.config.pick_filters = on;
                                // Off takes back a filter nothing here could
                                // lift any more.
                                if on {
                                    this.publish_picks(cx);
                                } else {
                                    this.drop_artist_filter(cx);
                                }
                                this.rebuild(cx);
                            },
                            cx,
                        ),
                    ),
                ))
                .child(crate::query::shared_query::search_section(
                    self.config.search,
                    |this: &mut Self, on, cx| {
                        this.config.search = on;
                        this.rebuild(cx);
                        this.refresh_title_bar(cx);
                    },
                    self.config.query_source,
                    |this: &mut Self, source, cx| this.pick_query_source(source, cx),
                    cx,
                ))
                .child(panel::tracking_section(
                    self.config.follow_playing,
                    rox_i18n::t!("artist-grid-follow-description"),
                    |this: &mut Self, on, cx| {
                        this.config.follow_playing = on;
                        if on {
                            this.follow_playing(cx);
                        }
                        cx.notify();
                    },
                    self.config.resume_playing,
                    rox_i18n::t!("artist-grid-resume-description"),
                    |this: &mut Self, on, cx| {
                        this.config.resume_playing = on;
                        cx.notify();
                    },
                    self.config.smooth_follow,
                    rox_i18n::t!("artist-grid-smooth-description"),
                    |this: &mut Self, on, cx| {
                        this.config.smooth_follow = on;
                        cx.notify();
                    },
                    cx,
                ))
                .child(settings_ui::section(
                    rox_i18n::t!("grid-section-dimming"),
                    None,
                    div()
                        .flex()
                        .flex_col()
                        .gap(tokens::SPACE_MD)
                        .child(setting_row(
                            rox_i18n::t!("artist-grid-dim-while-playing"),
                            Some(rox_i18n::t!("artist-grid-dim-while-playing.description")),
                            toggle(
                                self.config.dim_playing,
                                |this: &mut Self, on, cx| {
                                    this.config.dim_playing = on;
                                    this.dim_fading = true;
                                    cx.notify();
                                },
                                cx,
                            ),
                        ))
                        .when(self.config.dim_playing, |d| {
                            d.child(setting_row(
                                rox_i18n::t!("wall-dim-amount"),
                                Some(rox_i18n::t!("wall-dim-amount.description")),
                                settings_ui::scalar(
                                    &self.dim_scrub,
                                    &self.value_edit,
                                    self.config.dim,
                                    settings_ui::span(0., TILE_DIM_MAX, "%").hard(),
                                    |this: &mut Self, value, cx| {
                                        this.config.dim = value;
                                        this.dim_fading = true;
                                        cx.notify();
                                    },
                                    cx,
                                ),
                            ))
                        })
                        .child(setting_row(
                            rox_i18n::t!("artist-grid-desaturate"),
                            Some(rox_i18n::t!("artist-grid-desaturate.description")),
                            toggle(
                                self.config.desaturate_playing,
                                |this: &mut Self, on, cx| {
                                    this.config.desaturate_playing = on;
                                    cx.notify();
                                },
                                cx,
                            ),
                        ))
                        .when(
                            self.config.dim_playing || self.config.desaturate_playing,
                            |d| {
                                d.child(setting_row(
                                    rox_i18n::t!("wall-dim-always"),
                                    Some(rox_i18n::t!("wall-dim-always.description")),
                                    toggle(
                                        self.config.dim_always,
                                        |this: &mut Self, on, cx| {
                                            this.config.dim_always = on;
                                            this.dim_fading = true;
                                            cx.notify();
                                        },
                                        cx,
                                    ),
                                ))
                            },
                        ),
                ))
                .into_any_element(),
        )
    }

    fn appearance(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rounding = self.config.rounding;
        Some(
            settings_ui::section(
                rox_i18n::t!("grid-section-tiles"),
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(setting_row(
                        rox_i18n::t!("artist-grid-portraits"),
                        Some(rox_i18n::t!("artist-grid-portraits.description")),
                        toggle(
                            self.config.portraits,
                            |this: &mut Self, on, cx| {
                                this.config.portraits = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        rox_i18n::t!("artist-grid-show-names"),
                        Some(rox_i18n::t!("artist-grid-show-names.description")),
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
                            rox_i18n::t!("wall-name-alignment"),
                            Some(rox_i18n::t!("wall-name-alignment.description")),
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
                        rox_i18n::t!("wall-show-counts"),
                        Some(rox_i18n::t!("wall-show-counts.description")),
                        toggle(
                            self.config.counts,
                            |this: &mut Self, on, cx| {
                                this.config.counts = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        rox_i18n::t!("grid-letter-rail"),
                        Some(rox_i18n::t!("grid-letter-rail.description")),
                        toggle(
                            self.config.letters,
                            |this: &mut Self, on, cx| {
                                this.config.letters = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .when(self.config.letters, |d| {
                        let side_icons: &'static [(&'static str, LetterSide)] =
                            if self.axis() == Axis::Horizontal {
                                &[
                                    (icons::PANEL_TOP, LetterSide::Start),
                                    (icons::PANEL_BOTTOM, LetterSide::End),
                                ]
                            } else {
                                &[
                                    (icons::PANEL_LEFT, LetterSide::Start),
                                    (icons::PANEL_RIGHT, LetterSide::End),
                                ]
                            };
                        d.child(setting_row(
                            rox_i18n::t!("letter-rail-compact"),
                            Some(rox_i18n::t!("letter-rail-compact.description")),
                            toggle(
                                self.config.letters_compact,
                                |this: &mut Self, on, cx| {
                                    this.config.letters_compact = on;
                                    cx.notify();
                                },
                                cx,
                            ),
                        ))
                        .child(setting_row(
                            rox_i18n::t!("letter-rail-side"),
                            Some(rox_i18n::t!("letter-rail-side.description")),
                            panel::icon_choices(
                                side_icons,
                                self.config.letters_side,
                                |this: &mut Self, side, cx| {
                                    this.config.letters_side = side;
                                    cx.notify();
                                },
                                cx,
                            ),
                        ))
                    })
                    .child(setting_row(
                        rox_i18n::t!("wall-tile-size"),
                        Some(rox_i18n::t!("wall-tile-size.description")),
                        settings_ui::scalar(
                            &self.tile_scrub,
                            &self.value_edit,
                            self.config.tile,
                            settings_ui::span(TILE_MIN, TILE_MAX, " px"),
                            |this: &mut Self, value, cx| {
                                this.config.tile = value;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        rox_i18n::t!("wall-gap"),
                        Some(rox_i18n::t!("wall-gap.description")),
                        settings_ui::scalar(
                            &self.gap_scrub,
                            &self.value_edit,
                            self.config.gap,
                            settings_ui::span(0., TILE_GAP_MAX, " px"),
                            |this: &mut Self, value, cx| {
                                this.config.gap = value;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        rox_i18n::t!("wall-rounding"),
                        Some(rox_i18n::t!("wall-rounding.description")),
                        settings_ui::scalar(
                            &self.rounding_scrub,
                            &self.value_edit,
                            rounding,
                            settings_ui::span(0., TILE_ROUNDING_MAX, "%").hard(),
                            |this: &mut Self, value, cx| {
                                this.config.rounding = value;
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

impl EventEmitter<PanelEvent> for ArtistGridPanel {}

impl Focusable for ArtistGridPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl QueryFilter for ArtistGridPanel {
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
        self.refresh_title_bar(cx);
    }
}

impl Panel for ArtistGridPanel {
    fn panel_name(&self) -> &'static str {
        "artist grid"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("panel-title-artists"),
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

    /// The wall serves its own tile menus, so the tab panel's body
    /// right-click stays out.
    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

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
        let weak_c = cx.entity().downgrade();
        let follow = self.config.follow_playing;
        let picked = !self.selected.is_empty();
        // Checks on the right, or the default left side swaps icons out for
        // the checkmark.
        let menu = menu
            .check_side(Side::Right)
            .item(
                PopupMenuItem::new(rox_i18n::t!("artist-grid-clear-picked"))
                    .icon(Icon::default().path(icons::CLOSE))
                    .disabled(!picked)
                    .on_click(move |_, _, cx| {
                        if let Some(this) = weak_c.upgrade() {
                            this.update(cx, |this, cx| this.clear_picks(cx));
                        }
                    }),
            )
            .separator()
            .item(
                PopupMenuItem::new(rox_i18n::t!("grid-jump-to-playing"))
                    .icon(Icon::default().path(icons::DISC))
                    .on_click(move |_, _, cx| {
                        if let Some(this) = weak.upgrade() {
                            this.update(cx, |this, cx| this.jump_to_playing(cx));
                        }
                    }),
            )
            .item(
                PopupMenuItem::new(rox_i18n::t!("tracking-follow"))
                    .icon(Icon::default().path(icons::LOCATE))
                    .checked(follow)
                    .on_click(move |_, _, cx| {
                        if let Some(this) = weak_f.upgrade() {
                            this.update(cx, |this, cx| this.toggle_follow_playing(cx));
                        }
                    }),
            );

        let menu = menu.separator().label(rox_i18n::t!("library-menu-display"));
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            submenu = submenu.check_side(Side::Right);
            for (name, icon, is_vertical) in [
                (
                    rox_i18n::t!("grid-vertical-scroll"),
                    icons::MOVE_VERTICAL,
                    true,
                ),
                (
                    rox_i18n::t!("grid-horizontal-scroll"),
                    icons::MOVE_HORIZONTAL,
                    false,
                ),
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
        let menu = menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("grid-menu-scroll"),
            submenu,
        ));
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            submenu = submenu.check_side(Side::Right);
            for group in [ArtistGroup::AlbumArtist, ArtistGroup::Artist] {
                submenu = submenu.item(panel::check_row(
                    group.label(),
                    None,
                    move |this: &Self| this.config.group == group,
                    move |this, cx| this.set_group(group, cx),
                    &panel,
                ));
            }
            submenu
        });
        let menu = menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("artist-grid-group-mode"),
            submenu,
        ));
        let panel = cx.entity();
        let menu = menu.item(panel::check_row(
            rox_i18n::t!("artist-grid-portraits"),
            Some(icons::USER),
            |this: &Self| this.config.portraits,
            |this, cx| {
                this.config.portraits = !this.config.portraits;
                cx.notify();
            },
            &panel,
        ));
        let menu = crate::query::shared_query::search_flyout(
            menu,
            |this: &Self| this.config.query_source,
            |this: &Self| this.config.search,
            &cx.entity(),
            |this, source, cx| this.pick_query_source(source, cx),
            |this, on, cx| {
                this.config.search = on;
                this.rebuild(cx);
                this.refresh_title_bar(cx);
            },
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
                ArtistGridPanel::new(state, config, window, cx)
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

impl Render for ArtistGridPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl ArtistGridPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.resync_box {
            self.resync_box = false;
            self.sync_query_box(window, cx);
        }
        let axis = self.axis();
        let lanes = self.lanes();
        let line_count = self.cells.len().div_ceil(lanes);
        let side = self.tile_side();

        // Coast and glide step here and request frames only while moving.
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
        // The lane count is only known after the first paint, so restore
        // waits for it. A running follow glide owns the position.
        if let Some(cell) = self.restore
            && self.glide_to.is_none()
            && !self.cells.is_empty()
            && self.cross > px(0.)
        {
            let line = (cell / lanes).min(line_count.saturating_sub(1));
            self.scroll.scroll_to_item(line, ScrollStrategy::Top);
            self.restore = None;
        }
        // The dim and portrait fades share one pass, gated so a settled wall
        // skips it on idle renders.
        if self.dim_fading || self.face_fading {
            let step = 1.0 - (0.08_f32).powf(dt * 10.0);
            let (dim_on, face_on) = (self.dim_fading, self.face_fading);
            let (mut dimming, mut fading) = (false, false);
            // An exponential approach never arrives, so snap when close.
            let settle = |current: f32, target: f32, moving: &mut bool| {
                let diff = target - current;
                if diff.abs() < 0.005 {
                    target
                } else {
                    *moving = true;
                    current + diff * step
                }
            };
            for ix in 0..self.cells.len() {
                if dim_on && let Some(current) = self.cells[ix].dim {
                    let target = self.dim_target(ix);
                    self.cells[ix].dim = Some(settle(current, target, &mut dimming));
                }
                if face_on && let Some(current) = self.cells[ix].face {
                    let target = if self.cells[ix].faced { 1. } else { 0. };
                    self.cells[ix].face = Some(settle(current, target, &mut fading));
                }
            }
            self.dim_fading = dimming;
            self.face_fading = fading;
            if dimming || fading {
                window.request_animation_frame();
            }
        }

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
            // Bindings beat key listeners, so a key the workspace binds never
            // reaches on_panel_key unless a context scopes it out. PanelNav
            // takes back left and right; TypeAhead adds space and tab while a
            // phrase is up.
            .key_context(panel::panel_nav_context(
                &self.type_ahead,
                self.type_ahead_at,
            ))
            // Any press ends the phrase. Capture phase, so tiles that stop
            // the press can't hide it.
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                this.clear_type_ahead(cx);
            }))
            .on_action(cx.listener(|this, _: &TypeAheadNext, _, cx| this.type_step(false, cx)))
            .on_action(cx.listener(|this, _: &TypeAheadPrev, _, cx| this.type_step(true, cx)))
            // The guard skips the search box's keys, which bubble up here.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.focus.is_focused(window) {
                    this.on_panel_key(event, cx);
                }
            }))
            .when(headerless && self.config.search, |d| {
                d.child(self.toolbar(cx))
            });
        // The "open a folder" prompt keys off the projection, never the view.
        let busy = self.state.library.read(cx).busy().is_some();
        let catalog_empty = self
            .state
            .library
            .read(cx)
            .projection()
            .is_some_and(|p| p.is_empty());
        let content: AnyElement = if catalog_empty && !busy {
            div()
                .id("artist-grid-empty")
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(tokens::SPACE_SM)
                .p(tokens::SPACE_MD)
                .text_center()
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    crate::catalog::browse(&this.state.library, cx);
                }))
                .child(div().text_lg().child(rox_i18n::t!("library-empty-title")))
                .child(
                    div()
                        .text_color(palette::text_muted())
                        .child(rox_i18n::t!("library-empty-note")),
                )
                .into_any_element()
        } else if self.cells.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .p(tokens::SPACE_MD)
                .text_center()
                .text_color(palette::text_muted())
                .child(
                    if self.effective_query(cx).is_empty() && self.browse_filter(cx).is_empty() {
                        rox_i18n::t!("grid-library-empty")
                    } else {
                        rox_i18n::t!("picker-no-matches")
                    },
                )
                .into_any_element()
        } else {
            let entity = cx.entity();
            // A horizontal wall stacks the caption in the cross extent, so
            // only a vertical one adds it to the scroll pitch.
            let line_extent = if self.config.vertical {
                side + px(self.label_height())
            } else {
                side
            };
            let item_sizes: Rc<Vec<Size<Pixels>>> =
                Rc::new(vec![size(side, line_extent); line_count]);
            let list = match axis {
                Axis::Vertical => {
                    v_virtual_list(entity, "artist-grid", item_sizes, |this, range, _, cx| {
                        this.lines(range, cx)
                    })
                }
                Axis::Horizontal => {
                    h_virtual_list(entity, "artist-grid", item_sizes, |this, range, _, cx| {
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
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        window.focus(&this.focus);
                        this.glide_to = None;
                        this.restore = None;
                        this.letter_hold = None;
                        this.flick.begin(event.position.along(axis));
                        this.touch_resume(cx);
                        cx.notify();
                    }),
                )
                // Only stamps the idle clock; the list does the scrolling.
                .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, _, cx| {
                    this.letter_hold = None;
                    this.touch_resume(cx);
                }))
                // gpui never cross-maps a plain wheel's y onto a horizontal
                // list, so fill that gap. Trackpad x deltas stay with the
                // list's handler, so nothing applies twice.
                .when(axis == Axis::Horizontal, |d| {
                    d.on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, window, cx| {
                        let delta = event.delta.pixel_delta(window.line_height());
                        if delta.x != px(0.) || delta.y == px(0.) {
                            return;
                        }
                        this.glide_to = None;
                        this.restore = None;
                        this.letter_hold = None;
                        let base = this.scroll.base_handle().clone();
                        let offset = base.offset().apply_along(Axis::Horizontal, |x| x + delta.y);
                        base.set_offset(offset);
                        cx.notify();
                    }))
                })
                .child(list)
                // The drag-scroll's window handlers arm in a paint pass. The
                // lines closure can't arm them since it also runs in layout.
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
                .children(panel::type_ahead_overlay(
                    &self.type_ahead,
                    self.type_ahead_at,
                ))
                // Keyed off the hovered tile, since the builder gets no
                // position. Outside the picks it repicks that tile first.
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
                                this.publish(cx);
                                cx.notify();
                            }
                            let mut ixs: Vec<usize> = this.selected.iter().copied().collect();
                            ixs.sort_unstable();
                            ixs
                        });
                        let label = if ixs.len() > 1 {
                            rox_i18n::t!("artist-grid-play-artists", count = ixs.len() as u64)
                                .to_string()
                        } else {
                            rox_i18n::t!("library-play").to_string()
                        };
                        let ids: Vec<i64> = this.update(cx, |this, cx| {
                            ixs.iter()
                                .flat_map(|&ix| this.ids_for(ix, cx))
                                .take(QUEUE_CAP)
                                .collect()
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
                        this.update(cx, |this, cx| {
                            this.dropdown_menu(menu.separator(), window, cx)
                        })
                    }
                })
                .into_any_element()
        };
        // The rail gets its own gutter so the letters never overlay tiles.
        let content = match self.letter_rail(cx) {
            Some(gutter) => {
                let row = self.axis() == Axis::Vertical;
                let start = self.config.letters_side == LetterSide::Start;
                let base = div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .flex()
                    .map(|d| if row { d.flex_row() } else { d.flex_col() });
                let wall = div().flex_1().min_w_0().min_h_0().flex().child(content);
                if start {
                    base.child(gutter).child(wall)
                } else {
                    base.child(wall).child(gutter)
                }
                .into_any_element()
            }
            None => content,
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
