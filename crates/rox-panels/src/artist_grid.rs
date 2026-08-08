//! The artist grid panel: the catalog's people as a wall of tiles, square,
//! the lanes splitting the panel's cross extent evenly so the wall runs
//! edge to edge - the album grid's shape, one level up. Tiles wear the
//! artist's own portrait when the setting is on, otherwise the cover of
//! the first album on their shelf.
//!
//! One tile per credited album artist, the library's own grouping rule, so
//! a record's guests stay on the act that released it rather than earning
//! a tile per feature. A setting regroups on the track artist for when you
//! want those guest spots findable; see [`ArtistGroup`].
//!
//! Its point is picking. Clicking a tile writes the artist onto the shared
//! filter, the same field the filter panel's Artist column writes, so every
//! global-following panel narrows to that artist at once: the album grid
//! below shows their records, the library their tracks. The wall itself
//! leaves that field out of its own mask, the filter panel's column rule,
//! so picking never collapses the shelf you picked from. A double click
//! plays the artist instead.
//!
//! A per-view query narrows the wall the usual way, and the artist tally
//! under each name counts what the query left. Deliberately not a library
//! view mode: per the workspace rule, browsing surfaces are panels of
//! their own.

use std::collections::HashSet;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    canvas, div, img, prelude::*, px, size, svg, Along, AnyElement, App, Axis, Context, Div,
    Entity, EventEmitter, FocusHandle, Focusable, Image, KeyDownEvent, Modifiers, MouseButton,
    MouseDownEvent, MouseUpEvent, ObjectFit, Pixels, ScrollStrategy, ScrollWheelEvent,
    SharedString, Size, Subscription, WeakEntity, Window,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::scroll::Scrollbar;
use gpui_component::{h_virtual_list, v_virtual_list, Icon, Side, VirtualListScrollHandle};
use rox_core::fmt::plural;
use rox_core::QUEUE_CAP;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::TrackKey;
use rox_library::projection::{FilterField, FilterSet, Projection, SortKey, SymTable};
use rox_panel_kit::config::{default_true, is_zero};
use rox_panel_kit::wall::{default_dim, default_gap, WallLayout, TILE_DIM_MAX, TILE_LABEL_H};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::grid::TitleAlign;
use crate::panel::{
    self, setting_row, toggle, AppState, FlickState, PanelChrome, PanelSettings, ResumeIdle,
    ScrubState,
};
use crate::panel_settings;
use crate::query::search::{SearchBox, SearchEvent};
use crate::query::shared_query::{QueryFilter, QuerySource, SharedQueryEvent};
use crate::selection::SelectionEvent;
use crate::settings::ui as settings_ui;
use crate::thumbs::Thumb;

/// The tile size knob's range, in px, the album grid's scale. The strip's
/// top sits at the stored thumbnail's long side, so scrubbing never
/// upscales past what either store keeps; a typed size can, and goes soft
/// for it.
const TILE_MIN: f32 = 96.;
const TILE_MAX: f32 = 256.;

/// The tile rounding knob's ceiling, in percent of circular. Artists
/// default to the full circle, which is what tells a face apart from a
/// record sleeve at a glance.
const TILE_ROUNDING_MAX: f32 = 100.;

/// The tile gap knob's ceiling, the panel frame sliders' scale.
const TILE_GAP_MAX: f32 = 24.;

/// How many columns the wall falls back to before its first paint has
/// measured a width.
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

/// Which name a tile stands for. The credited album artist by default, the
/// library's own grouping rule: one shelf per act, guests and features
/// folded onto the record they appear on. The track artist splits those
/// back out, so every "feat." credit earns a tile of its own - a far
/// longer wall, and the one to pick when you want a guest spot findable.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtistGroup {
    #[default]
    AlbumArtist,
    Artist,
}

impl ArtistGroup {
    fn label(self) -> &'static str {
        match self {
            ArtistGroup::AlbumArtist => "Album Artist",
            ArtistGroup::Artist => "Track Artist",
        }
    }

    /// The filter field a pick writes, the same values the filter panel's
    /// matching column writes.
    fn field(self) -> FilterField {
        match self {
            ArtistGroup::AlbumArtist => FilterField::AlbumArtist,
            ArtistGroup::Artist => FilterField::Artist,
        }
    }

    /// The projection column the runs break on and the table its symbols
    /// name.
    fn source(self, projection: &Projection) -> (&[u32], &SymTable) {
        match self {
            ArtistGroup::AlbumArtist => (&projection.album_artist, &projection.album_artists),
            ArtistGroup::Artist => (&projection.artist, &projection.artists),
        }
    }
}

/// The artist grid's per-view config: what a saved layout restores, and
/// what the settings window edits.
#[derive(Clone, Serialize, Deserialize)]
pub struct ArtistGridConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub query: String,
    /// Show the search box; the query only applies while it shows.
    #[serde(default)]
    pub search: bool,
    /// Whether this wall filters by its own query or follows the shared
    /// app-wide one.
    #[serde(default)]
    pub query_source: QuerySource,
    /// Which name a tile stands for: the credited album artist, or the
    /// track artist with every feature split back out.
    #[serde(default)]
    pub group: ArtistGroup,
    /// Scroll the wall vertically, rows filling the width; off scrolls it
    /// horizontally, columns filling the height.
    #[serde(default = "default_true")]
    pub vertical: bool,
    /// The preferred tile edge in px. The strip picks inside
    /// [`TILE_MIN`]..[`TILE_MAX`]; a typed size reaches past the top.
    #[serde(default = "default_tile")]
    pub tile: f32,
    /// Picking an artist writes them onto the shared filter, so every
    /// panel following the shared query narrows to them. On by default:
    /// driving the rest of the workspace is what this wall is for. Off
    /// leaves the pick as a plain selection, the album grid's behavior.
    #[serde(default = "default_true")]
    pub pick_filters: bool,
    /// Show the artist's own portrait instead of an album cover, fetched
    /// once per name and cached. Off by default; a wall of a thousand
    /// unknown names shouldn't reach for the network unasked.
    #[serde(default)]
    pub portraits: bool,
    /// Scroll to the playing artist when the track changes.
    #[serde(default)]
    pub follow_playing: bool,
    /// After the wall sits untouched for a spell, slide back to the
    /// playing artist on its own.
    #[serde(default)]
    pub resume_playing: bool,
    /// Glide there instead of jumping.
    #[serde(default)]
    pub smooth_follow: bool,
    /// While a track plays, fade every tile but the playing artist's;
    /// hovering lights a tile back up.
    #[serde(default)]
    pub dim_playing: bool,
    /// The same focus effect in color: drain every tile but the playing
    /// artist's to grayscale.
    #[serde(default)]
    pub desaturate_playing: bool,
    /// Keep the dim and desaturate effects on all the time, not only while
    /// a track plays.
    #[serde(default)]
    pub dim_always: bool,
    /// How far the dimmed tiles fade, in percent of fully hidden.
    #[serde(default = "default_dim")]
    pub dim: f32,
    /// Each tile's corner rounding, in percent of circular.
    #[serde(default = "default_rounding")]
    pub rounding: f32,
    /// The space between tiles, in px.
    #[serde(default = "default_gap")]
    pub gap: f32,
    /// Print the artist's name under their tile.
    #[serde(default = "default_true")]
    pub labels: bool,
    /// How those captions line up under their tiles.
    #[serde(default)]
    pub label_align: TitleAlign,
    /// The album and track tally under the name.
    #[serde(default = "default_true")]
    pub counts: bool,
    /// The top-left artist shown when the layout was saved, so a relaunch
    /// reopens the wall where it was left. A cell index, so it survives a
    /// tile-size or width change.
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

/// One artist's run in the current view: which name, where it starts, how
/// many tracks and albums it spans, and the cover path once a paint
/// resolved it (the inner None is an artist with nothing to show).
struct Cell {
    /// The interned symbol in whichever table the wall groups by: the run's
    /// key, and what the name and the filter value read off.
    sym: u32,
    start: usize,
    len: u32,
    /// Distinct albums in the run, the caption's tally.
    albums: u32,
    art: Option<Option<PathBuf>>,
    /// The tile's current opacity under the dim mode, easing toward its
    /// target every frame. None until the tile's first paint, which lands
    /// at the target directly.
    dim: Option<f32>,
    /// How far the artist's face has come in over the album cover: 0 is the
    /// cover alone, 1 the portrait alone, in between the two crossfaded.
    /// None until the tile's first paint, which lands at its target
    /// directly - a portrait already in hand arrives solid rather than
    /// fading in again on every rebuild.
    face: Option<f32>,
    /// Whether the artist's portrait is in hand, learned on the tile's own
    /// paint. The fade loop reads this instead of re-probing the cache by
    /// name every frame, which would fold a thousand names per frame on a
    /// big wall.
    faced: bool,
}

pub struct ArtistGridPanel {
    state: AppState,
    config: ArtistGridConfig,
    /// The rows the cells index into: the canonical order while nothing
    /// narrows it, otherwise the hits re-ordered canonically so an
    /// artist's tracks stay one contiguous run.
    view: Arc<Vec<u32>>,
    /// The artists of the current view, rebuilt on library updates and
    /// query changes.
    cells: Vec<Cell>,
    /// The query editor, the shared search box; `config.query` mirrors its
    /// value via change events.
    search: Entity<SearchBox>,
    /// The picked artists, the accent outlines and the shared filter's
    /// values. While `pick_filters` is on this mirrors the filter, so a
    /// chip cleared in the search bar lifts the outline here too.
    selected: HashSet<usize>,
    /// Where a shift-extend grows from: the last plain or toggle click.
    anchor: Option<usize>,
    /// The tile under the pointer, wearing the name overlay.
    hovered: Option<usize>,
    /// The cross extent the wall last laid out for: the width while it
    /// scrolls vertically, the height otherwise. The dock hosts panels
    /// cached, so a resize repaints without re-rendering; the list closure
    /// compares the painted extent against this and notifies on drift.
    cross: Pixels,
    scroll: VirtualListScrollHandle,
    /// The drag-to-scroll state: press anywhere on the wall, drag to
    /// scroll, release to coast. A drag past its dead zone swallows the
    /// tile click.
    flick: FlickState,
    /// The list row the follow-playing glide is headed to.
    glide_to: Option<usize>,
    /// The saved top-left artist waiting to be scrolled back into place on
    /// a relaunch, held until the wall has both artists and a measured
    /// width. A user drag clears it, so a hand on the wall wins.
    restore: Option<usize>,
    /// The last animation tick, the coast's and the glide's dt.
    last_tick: Instant,
    /// The idle-resume clock, stamped on every scroll or press.
    resume_idle: ResumeIdle,
    /// The playing track's path, the change detector for follow-playing.
    playing_key: Option<TrackKey>,
    /// The playing artist's cell in the current view, kept fresh so
    /// per-frame dimming never rescans.
    playing_ix: Option<usize>,
    /// Whether audio is moving right now; pause lifts the dim.
    playing: bool,
    /// A dim fade is in flight, so the per-frame ease loop should run.
    dim_fading: bool,
    /// A portrait crossfade is in flight, the same gate for the same loop.
    /// Armed by the tile that first sees a face it hasn't faded in yet.
    face_fading: bool,
    /// The tile size slider's scrub strip, for the settings window.
    tile_scrub: ScrubState,
    /// The tile rounding slider's scrub strip, same window.
    rounding_scrub: ScrubState,
    /// The tile gap slider's scrub strip, same window.
    gap_scrub: ScrubState,
    /// The dim amount slider's scrub strip, the behavior page.
    dim_scrub: ScrubState,
    /// The one readout being typed into across the settings sliders.
    value_edit: panel::ValueEdit,
    /// A failed play, shown in a strip until the next one lands.
    error: Option<SharedString>,
    /// A pending box reset from a source toggle or a shared-query change;
    /// applied on the next render, where a window exists to set the input.
    resync_box: bool,
    /// The tracks this panel is pinned to while following the selection.
    /// Runtime only: a restore re-pins from whatever is picked then.
    selection_ids: Vec<i64>,
    /// The type-ahead phrase and when its last keystroke landed, so typing
    /// while the wall has focus jumps to the artist by prefix.
    type_ahead: String,
    type_ahead_at: Option<Instant>,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and
    /// pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _library_changed: Subscription,
    _thumbs_changed: Subscription,
    /// Landing faces notify the shared service; repaint so tiles fill in.
    _portraits_changed: Subscription,
    _search_events: Subscription,
    _query_changed: Subscription,
    _selection_changed: Subscription,
    _player_changed: Subscription,
}

impl ArtistGridPanel {
    pub fn new(
        state: AppState,
        config: ArtistGridConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // A rescan can rewrite the order, tags, and id -> path mappings;
        // rebuild the artists over the new projection.
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
        // Landing thumbnails notify the service; repaint so tiles fill in.
        let _thumbs_changed = cx.observe(&state.thumbs, |_, _, cx| cx.notify());
        let _portraits_changed = cx.observe(&state.portraits, |_, _, cx| cx.notify());
        // A wall restored as global opens showing the shared query; a local
        // one shows its own.
        let initial = match config.query_source {
            QuerySource::Global => state.query.read(cx).text().to_string(),
            QuerySource::Local | QuerySource::Selection => config.query.clone(),
        };
        let search = cx.new(|cx| SearchBox::new("Search", &initial, window, cx).small());
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        // The shared query and the shared filter both land here: our own
        // picks come back around this way, which is what keeps the outlines
        // honest when another surface clears them.
        let _query_changed = cx.subscribe(
            &state.query,
            |this: &mut Self, _, _: &SharedQueryEvent, cx| {
                this.on_shared_query_changed(cx);
            },
        );
        // Restored as selection-following, it opens on whatever is picked
        // now, rather than blank until the next pick.
        let selection_ids = state.selection.read(cx).tracks().to_vec();
        // Follow the app-wide selection while pinned to it.
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
        // scroll; every other panel restores where it was left.
        let restore = (!config.follow_playing && config.scroll > 0).then_some(config.scroll);
        let mut this = ArtistGridPanel {
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
            focus: cx.focus_handle(),
            tab_panel: None,
            _library_changed,
            _thumbs_changed,
            _portraits_changed,
            _search_events,
            _query_changed,
            _selection_changed,
            _player_changed,
        };
        this.rebuild(cx);
        // A duplicate opens with a track already playing; pick it up now
        // instead of waiting for the next track change.
        this.sync_playing(cx);
        this
    }

    /// Follow the player: on a track change, head for the artist it lives
    /// under, and keep the dim mode's facts fresh. The compares keep the
    /// per-tick observer cheap, the player notifies every pump.
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
        // The un-dimmed artist moved, so the old and new tiles both ease.
        self.dim_fading = true;
        if self.config.follow_playing {
            self.follow_playing(cx);
        }
        cx.notify();
    }

    /// The playing track's artist in the current view, when it holds one.
    fn playing_cell(&self, cx: &App) -> Option<usize> {
        let key = self.playing_key.as_ref()?;
        let library = self.state.library.read(cx);
        let id = library.id_for_key(key)?;
        let projection = library.projection()?;
        let view_ix = self
            .view
            .iter()
            .position(|&row| projection.db_id[row as usize] == id)?;
        // Cells are contiguous runs over the view; the last one starting
        // at or before the hit holds it.
        Some(
            self.cells
                .partition_point(|cell| cell.start <= view_ix)
                .saturating_sub(1),
        )
    }

    /// Scroll the playing artist into view: a glide when smooth is on, a
    /// centered jump otherwise.
    fn follow_playing(&mut self, cx: &mut Context<Self>) {
        let Some(cell_ix) = self.playing_ix else {
            return;
        };
        // Both modes head for the same line through the per-frame stepping
        // in `body`: the line is the stable fact, its offset depends on a
        // layout that may still be settling.
        self.glide_to = Some(cell_ix / self.lanes());
        cx.notify();
    }

    /// The menu's jump: pick the playing artist and head there with the
    /// panel's configured motion. The automatic follow never touches the
    /// picks; this deliberate move does.
    fn jump_to_playing(&mut self, cx: &mut Context<Self>) {
        let Some(cell_ix) = self.playing_ix else {
            return;
        };
        self.selected = HashSet::from([cell_ix]);
        self.anchor = Some(cell_ix);
        self.publish(cx);
        self.follow_playing(cx);
    }

    /// A scroll, drag, or press: restart the idle clock and arm a wake, so
    /// the wall drifts back to the playing artist once the user steps away.
    fn touch_resume(&mut self, cx: &mut Context<Self>) {
        if self.config.resume_playing {
            self.resume_idle.touch(cx, Self::resume_to_playing);
        }
    }

    /// The idle wake's landing: slide back to the playing artist, so long
    /// as the resume is still on.
    fn resume_to_playing(&mut self, cx: &mut Context<Self>) {
        if self.config.resume_playing {
            self.follow_playing(cx);
        }
    }

    /// The menu's follow toggle: flip the follow state and catch up right
    /// away when turning it on.
    fn toggle_follow_playing(&mut self, cx: &mut Context<Self>) {
        self.config.follow_playing = !self.config.follow_playing;
        if self.config.follow_playing {
            self.follow_playing(cx);
        }
        cx.notify();
    }

    /// Flip the scroll axis, from the context menu or the settings toggle.
    /// The lane count and tile edge both key off the cross extent, so drop
    /// the measured one and let the next paint re-measure.
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

    /// The filter the wall narrows itself by: the shared picks with its own
    /// field taken out. The filter panel's column rule, and the reason
    /// picking here works at all - a wall that honored its own pick would
    /// collapse to the one tile you just clicked.
    fn browse_filter(&self, cx: &App) -> FilterSet {
        let mut filter = self.effective_filter(cx);
        filter.clear(self.config.group.field());
        filter
    }

    /// Switch which name a tile stands for. The picks name values in the
    /// field the wall is leaving, so they go with it rather than leaving
    /// the workspace narrowed by a shelf this wall no longer shows.
    fn set_group(&mut self, group: ArtistGroup, cx: &mut Context<Self>) {
        if self.config.group == group {
            return;
        }
        self.drop_artist_filter(cx);
        self.config.group = group;
        self.rebuild(cx);
    }

    /// Recompute the view and its artist runs, cut to the query's hits and
    /// the shared filter's other fields. Search hits come back in
    /// projection row order, so they filter an ordered view rather than
    /// getting walked directly - otherwise an artist's scattered rows would
    /// split into duplicate tiles.
    ///
    /// Which order depends on what a tile stands for. The canonical one
    /// already groups by album artist, so its runs are the album-artist
    /// wall's tiles for free. A track-artist wall needs its own: a guest
    /// turns up under every act they recorded with, and those rows sit a
    /// shelf apart in the canonical order, so one name would scatter into a
    /// tile per host album.
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
                        // Ties fall back to the canonical order, so an
                        // artist's own run still reads album by album.
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
                // The canonical order sorts album within artist, so a
                // change of album symbol inside the run is a new record.
                if last_album != Some(album) {
                    cell.albums += 1;
                    last_album = Some(album);
                }
            }
        }
        // A pick writes the shared filter, which comes back around as this
        // very rebuild; keeping the anchor and the hover across it is what
        // stops a click from breaking shift-extend or blinking the name
        // overlay off. Both are clamped, so a query that shortened the wall
        // can't leave them pointing off the end.
        self.anchor = self.anchor.filter(|&ix| ix < self.cells.len());
        self.hovered = self.hovered.filter(|&ix| ix < self.cells.len());
        self.sync_picks(cx);
        self.playing_ix = self.playing_cell(cx);
        cx.notify();
    }

    /// Re-derive the outlined tiles from the shared filter's artist picks.
    /// While the wall drives the filter that is the one source of truth, so
    /// a chip cleared in the search bar or a value unticked in the filter
    /// panel lifts the outline here without a word between the panels.
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

    /// Map the shared box's events onto the wall: a changed query rebuilds
    /// the view, and every visual change also repaints the title row, which
    /// only updates when the tab panel is notified.
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

    /// An artist's name, the caption and the filter value both. Empty off
    /// the end of the cells or before a projection lands.
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

    /// An artist's tracks as db ids in view order, capped for the player
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

    /// The path a tile's cover loads by: the first track under the artist
    /// that carries an album tag, resolved through the store once, on the
    /// tile's first paint. The untagged bucket has no record of its own, so
    /// a loose track's art never stands in for a whole shelf.
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

    /// An artist's portrait through the shared service, which owns the
    /// cache, the lookup pool, and the eviction; None is a face still on
    /// its way or one no service knows, and the tile falls back to an
    /// album cover either way.
    fn portrait(&mut self, ix: usize, cx: &mut Context<Self>) -> Option<Arc<Image>> {
        let name = self.cell_name(ix, cx);
        let portraits = self.state.portraits.clone();
        portraits.update(cx, |portraits, cx| portraits.get(&name, cx))
    }

    /// Put a click on an artist tile: plain picks just them, shift extends
    /// from the anchor, cmd (ctrl elsewhere) toggles - the library's click
    /// rules, by tile. Publishes the picks either way.
    fn select(&mut self, ix: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        if modifiers.shift {
            let anchor = self.anchor.unwrap_or(ix);
            let (lo, hi) = (anchor.min(ix), anchor.max(ix));
            // Ctrl+Shift stacks the range onto the picks so you can skip a
            // run and grab a second block; plain shift replaces.
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
            // A second plain click on the only picked artist drops the pick,
            // so the wall is its own way back to the whole catalog.
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

    /// Send the picks out: their tracks on the shared selection, and the
    /// names themselves on the shared filter when this wall drives it.
    fn publish(&mut self, cx: &mut Context<Self>) {
        self.publish_selection(cx);
        self.publish_picks(cx);
    }

    /// Resolve the picked artists to db ids in view order and publish them
    /// on the shared selection, so the inspector panels follow a click here
    /// the same as one in the library.
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

    /// Write the picked names onto the shared filter's artist field, the
    /// same values the filter panel's Artist column writes. Every panel
    /// following the shared query narrows to them; this wall doesn't,
    /// having left the field out of its own mask.
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

    /// Drop every artist pick, the menu's reset: the outlines lift and
    /// every following panel widens back to the whole catalog.
    fn clear_picks(&mut self, cx: &mut Context<Self>) {
        self.selected.clear();
        self.anchor = None;
        self.publish_selection(cx);
        self.drop_artist_filter(cx);
        cx.notify();
    }

    /// Take the artist field off the shared filter. Unconditional, unlike
    /// [`Self::publish_picks`]: switching the picking behavior off has to
    /// lift a filter the wall can no longer reach, or the workspace stays
    /// narrowed with nothing left to widen it.
    fn drop_artist_filter(&mut self, cx: &mut Context<Self>) {
        let field = self.config.group.field();
        self.state.query.clone().update(cx, |query, cx| {
            let mut filter = query.filter().clone();
            filter.clear(field);
            query.set_filter(filter, cx);
        });
    }

    /// Browse from the keyboard while the wall is focused: plain typing
    /// jumps to the artist whose name starts with the phrase. Modifiers
    /// pass through so the workspace keeps its shortcuts, and a leading
    /// space stays its play/pause.
    fn on_panel_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.alt {
            return;
        }
        // Escape drops the picks. No select-all here: every artist picked
        // is the whole catalog anyway, and it would pour every name into
        // the shared filter.
        if keystroke.key.as_str() == "escape" {
            self.deselect(cx);
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

    /// Escape drops the picks: the shared selection empties and the
    /// filter names clear with it, the same road the single-click
    /// deselect takes.
    fn deselect(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            return;
        }
        self.selected.clear();
        self.anchor = None;
        self.publish(cx);
        cx.notify();
    }

    /// Grow or restart the type-ahead phrase and jump to the artist it
    /// names. A fresh phrase starts past the current pick, so the same
    /// letter walks to the next match; a grown one re-tests the current
    /// artist so refining a match stays put.
    fn type_to(&mut self, text: String, cx: &mut Context<Self>) {
        let grown = panel::type_ahead_grow(&mut self.type_ahead, &mut self.type_ahead_at, text);
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
                    self.cells
                        .get(ix)
                        .is_some_and(|cell| table.lower[cell.sym as usize].starts_with(&needle))
                })
            })
        };
        if let Some(ix) = hit {
            self.selected = HashSet::from([ix]);
            self.anchor = Some(ix);
            self.publish(cx);
            self.scroll_to_cell(ix, cx);
        }
    }

    /// Bring an artist's tile into view, centered on the scroll axis.
    /// Clears any pending glide or restore so the jump wins over an
    /// automatic move.
    fn scroll_to_cell(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.glide_to = None;
        self.restore = None;
        let line = ix / self.lanes();
        self.scroll.scroll_to_item(line, ScrollStrategy::Center);
        cx.notify();
    }

    /// Queue an artist on the shared player.
    fn play(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.play_many(vec![ix], cx);
    }

    /// Queue several artists on the shared player, in view order under the
    /// queue cap.
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
                    .update(cx, |player, cx| player.play_explicit(keys, cx));
            }
            Err(e) => {
                self.error = Some(format!("library: {e}").into());
                cx.notify();
            }
        }
    }

    /// The wall geometry and focus state this panel draws under, the packing
    /// math shared with the other tile walls.
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

    /// One artist tile: the portrait or cover filling a square, the name
    /// overlay while hovered, the accent outline while picked. Pending and
    /// missing art wear the same quiet placeholder, so a landing face fills
    /// the tile without a flash.
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
        // The artist's own face when there is one; an album cover carries
        // the tile until it lands, and stays for a name no service knows.
        let face = self
            .config
            .portraits
            .then(|| self.portrait(ix, cx))
            .flatten();
        // The crossfade's progress, seeded on the tile's first paint and
        // stepped by the loop in `body` after that. A face that was already
        // in hand seeds at 1, so only one that actually arrives late fades.
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
        // Tell the loop what this tile is holding, and wake it when the two
        // disagree. The notify is what turns a landed portrait into the next
        // frame; from there `body` requests its own until the fade settles.
        // The ease snaps the last hair onto its target, so the compare is
        // exact rather than a float that never quite arrives.
        if let Some(cell) = self.cells.get_mut(ix) {
            cell.faced = faced;
        }
        if faded != target && !self.face_fading {
            self.face_fading = true;
            cx.notify();
        }
        // The cover under the face: still wanted while it shows through, and
        // dropped once the portrait covers it, so a settled wall of faces
        // stops touching the thumbnail cache at all.
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
        // The knob is percent of circular, so the radius scales with the
        // tile: 100 turns the square into a circle. It clips the image
        // itself, not just the tile's background: gpui content masks stay
        // rectangular, so a rounded tile under a square image would paint
        // over its own corners.
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
        // The floor: the album cover, or the quiet placeholder when there is
        // none yet. Skipped once the face has fully covered it.
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
        // The face over it, its opacity the crossfade itself.
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
                    // Hovering lights a receded tile back up, so re-arm the
                    // ease loop to fade the dim off and back on.
                    this.dim_fading = true;
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
            .child(face_square)
            .when(labels, |d| d.child(self.caption(ix, side, picked, cx)))
            .into_any_element()
    }

    /// A tile's name and tally: the artist over what the current view
    /// holds of them.
    fn cell_labels(&self, ix: usize, cx: &App) -> (SharedString, SharedString) {
        let Some(cell) = self.cells.get(ix) else {
            return Default::default();
        };
        let name = self.cell_name(ix, cx);
        // An untagged shelf reads as Unknown, the filter panel's wording,
        // while the pick it writes stays the real empty string.
        let name = if name.is_empty() {
            "Unknown".to_string()
        } else {
            name
        };
        let tally = if self.config.counts {
            format!(
                "{}, {}",
                plural(cell.albums, "album"),
                plural(cell.len, "track")
            )
        } else {
            String::new()
        };
        (SharedString::from(name), SharedString::from(tally))
    }

    /// The hover overlay: name over tally on a translucent strip along the
    /// tile's bottom edge.
    fn label(&self, ix: usize, cx: &App) -> Div {
        let (name, tally) = self.cell_labels(ix, cx);
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
                    .child(name),
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

    /// The always-on caption under a tile: name over tally in a fixed
    /// block, so the tile's total height stays predictable for the virtual
    /// list. Widths match the tile so long names truncate at its edge, and
    /// a picked artist's name wears the accent so the wall still reads once
    /// the outline is off screen.
    fn caption(&self, ix: usize, side: Pixels, picked: bool, cx: &App) -> Div {
        let (name, tally) = self.cell_labels(ix, cx);
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
                .child(name),
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

    /// The visible rows of the wall, each a run of tiles. Also where the
    /// painted extent reconciles: the dock hosts panels cached, so a resize
    /// repaints this closure without re-running render, and a notify here
    /// is what recomputes the lane count next frame.
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
        // Warm the margin: ask for the covers just past both edges so a
        // scroll reveals loaded tiles. Asked after the visible tiles, which
        // keeps those first in line for the load pool's slots. Portraits
        // stay out of it - their pool is small and their fetches are
        // somebody else's bandwidth, so they only load for what shows.
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

    fn behavior(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        Some(
            div()
                .flex()
                .flex_col()
                .gap(settings_ui::SECTION_GAP)
                .child(settings_ui::section(
                    "Grouping",
                    None,
                    setting_row(
                        "One Tile Per",
                        Some("The credited album artist keeps a record's guests on the act that released it; the track artist splits every feature onto a tile of its own"),
                        panel::choices(
                            &[
                                ("Album Artist", ArtistGroup::AlbumArtist),
                                ("Track Artist", ArtistGroup::Artist),
                            ],
                            self.config.group,
                            |this: &mut Self, group, cx| this.set_group(group, cx),
                            cx,
                        ),
                    ),
                ))
                .child(settings_ui::section(
                    "Picking",
                    None,
                    setting_row(
                        "Pick Filters the Library",
                        Some("Clicking an artist narrows every panel following the shared search to them; off leaves the click as a plain selection"),
                        toggle(
                            self.config.pick_filters,
                            |this: &mut Self, on, cx| {
                                this.config.pick_filters = on;
                                // Turning it on hands the highlighted artists
                                // straight over; turning it off takes back a
                                // filter nothing here could lift any more.
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
                        this.rebuild(cx);
                        this.refresh_title_bar(cx);
                    },
                    self.config.query_source,
                    |this: &mut Self, source, cx| this.pick_query_source(source, cx),
                    cx,
                ))
                .child(panel::tracking_section(
                    self.config.follow_playing,
                    "Scroll to the playing artist whenever the track changes",
                    |this: &mut Self, on, cx| {
                        this.config.follow_playing = on;
                        if on {
                            this.follow_playing(cx);
                        }
                        cx.notify();
                    },
                    self.config.resume_playing,
                    "Slide back to the playing artist after you stop browsing",
                    |this: &mut Self, on, cx| {
                        this.config.resume_playing = on;
                        cx.notify();
                    },
                    self.config.smooth_follow,
                    "Glide to the artist instead of jumping",
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
                            Some("Fade every tile but the playing artist's; hovering lights a tile back up"),
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
                                "Dim Amount",
                                Some("How far the other tiles fade; 100% hides them"),
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
                            "Desaturate While Playing",
                            Some("Drain every tile but the playing artist's to grayscale; hovering brings a tile's color back"),
                            toggle(
                                self.config.desaturate_playing,
                                |this: &mut Self, on, cx| {
                                    this.config.desaturate_playing = on;
                                    cx.notify();
                                },
                                cx,
                            ),
                        ))
                        .when(self.config.dim_playing || self.config.desaturate_playing, |d| {
                            d.child(setting_row(
                                "Always",
                                Some("Keep the tiles pushed back even when nothing plays; only a hovered tile shows in full"),
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
                        }),
                ))
                .into_any_element(),
        )
    }

    /// The wall's own appearance rows on the shared page: what the tiles
    /// show and how they are shaped, look knobs that live on the config
    /// rather than the theme because they shape the art, not the panel
    /// frame.
    fn appearance(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rounding = self.config.rounding;
        Some(
            settings_ui::section(
                "Tiles",
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(setting_row(
                        "Artist Portraits",
                        Some("Show each artist's own picture, looked up once per name and kept on disk; off shows the first album's cover"),
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
                        "Show Names",
                        Some("Print the artist under every tile instead of only on hover"),
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
                            "Name Alignment",
                            Some("Line the captions up under their tiles"),
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
                        "Show Counts",
                        Some("The album and track tally under each name"),
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
                        "Tile Size",
                        Some("The tiles' widest edge; columns split the panel width evenly"),
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
                        "Gap",
                        Some("Space between the tiles"),
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
                        "Rounding",
                        Some("Round each tile's corners; 100% is a circle"),
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

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Artists")
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

    /// The wall serves tile context menus over the whole body, so the tab
    /// panel's body right-click stays out.
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

    /// The layout dump carries the panel's config; the builder registered
    /// in `workspace::register_panels` reads it back.
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
        // Checks on the right so the orientation pair keeps its icons; the
        // default left side would swap them out for the checkmark.
        let menu = menu
            .check_side(Side::Right)
            .item(
                PopupMenuItem::new("Clear Picked Artists")
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
        // stays short, the same shape as the album grid's.
        let menu = menu.separator().label("Display");
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
        // What a tile stands for, a checked pair so the current rule reads
        // at a glance.
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
        let menu = menu.item(PopupMenuItem::submenu("One Tile Per", submenu));
        let panel = cx.entity();
        let menu = menu.item(panel::check_row(
            "Artist Portraits",
            Some(icons::USER),
            |this: &Self| this.config.portraits,
            |this, cx| {
                this.config.portraits = !this.config.portraits;
                cx.notify();
            },
            &panel,
        ));
        // Follow the shared search query, or filter by this wall's own box.
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
        // glide eases toward its line. Both step here in render and request
        // the next frame only while something still moves.
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
        // Restore the saved scroll once the wall has artists and a measured
        // extent: the lane count only lands after the first paint, and the
        // cell -> line map rides on it. Skipped while a follow glide runs,
        // which owns the position.
        if let Some(cell) = self.restore {
            if self.glide_to.is_none() && !self.cells.is_empty() && self.cross > px(0.) {
                let line = (cell / lanes).min(line_count.saturating_sub(1));
                self.scroll.scroll_to_item(line, ScrollStrategy::Top);
                self.restore = None;
            }
        }
        // The two per-tile fades: the dim mode's opacity, and the crossfade
        // that swaps an album cover for the artist's face once it lands.
        // They share one pass over the cells, and the pass is gated on both
        // flags, so a settled wall skips it entirely on the idle renders
        // hover and scroll trigger.
        if self.dim_fading || self.face_fading {
            let step = 1.0 - (0.08_f32).powf(dt * 10.0);
            let (dim_on, face_on) = (self.dim_fading, self.face_fading);
            let (mut dimming, mut fading) = (false, false);
            // An exponential approach never quite arrives, so anything
            // inside this of its target snaps and stops asking for frames.
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
                if dim_on {
                    if let Some(current) = self.cells[ix].dim {
                        let target = self.dim_target(ix);
                        self.cells[ix].dim = Some(settle(current, target, &mut dimming));
                    }
                }
                if face_on {
                    if let Some(current) = self.cells[ix].face {
                        let target = if self.cells[ix].faced { 1. } else { 0. };
                        self.cells[ix].face = Some(settle(current, target, &mut fading));
                    }
                }
            }
            self.dim_fading = dimming;
            self.face_fading = fading;
            if dimming || fading {
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
            // Type-to-jump while the wall itself holds focus. The guard
            // keeps it off while the search box is focused, whose keys
            // bubble up through the toolbar child.
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.focus.is_focused(window) {
                    this.on_panel_key(event, cx);
                }
            }))
            .when(headerless && self.config.search, |d| {
                d.child(self.toolbar(cx))
            });
        // The "open a folder" call-to-action means the catalog itself holds
        // no tracks, so it keys off the loaded projection, never the view.
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
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    crate::catalog::browse(&this.state.library, cx);
                }))
                .child(div().text_lg().child("Open a music folder"))
                .child(
                    div()
                        .text_color(palette::text_muted())
                        .child("It gets scanned into the library (flac, mp3, wav)"),
                )
                .into_any_element()
        } else if self.cells.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(palette::text_muted())
                .child(
                    if self.effective_query(cx).is_empty() && self.browse_filter(cx).is_empty() {
                        "The library is empty"
                    } else {
                        "No matches"
                    },
                )
                .into_any_element()
        } else {
            let entity = cx.entity();
            // Each line spans the tile plus, on a vertical wall, the caption
            // that trails it into the scroll; a horizontal wall stacks the
            // caption inside the cross extent, so its scroll pitch stays the
            // bare tile width.
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
                // Any press on the wall might be a drag-scroll; the tiles'
                // own actions moved to release so both can tell. It also
                // interrupts a running glide, the user wins.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        window.focus(&this.focus);
                        this.glide_to = None;
                        this.restore = None;
                        this.flick.begin(event.position.along(axis));
                        this.touch_resume(cx);
                        cx.notify();
                    }),
                )
                // Every wheel over the wall counts as browsing; this stamp
                // only restarts the idle clock and leaves the scroll itself
                // to the list and the gap-filler below.
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
                // picks acts on the whole set, outside it the click repicks
                // just that tile first, so the menu always acts on what is
                // highlighted - the library's rule.
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
                            format!("Play {} Artists", ixs.len())
                        } else {
                            "Play".to_string()
                        };
                        // The picked artists' tracks as db ids, resolved now
                        // for the editors, the library rows' move.
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
