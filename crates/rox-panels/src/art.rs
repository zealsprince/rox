//! The art view panel: the catalog as a cover carousel, NekoRoX's shelf.
//! One album centered and square, its neighbors shrinking, turning, and
//! tucking behind it toward both edges, so browsing reads as flipping
//! through a rack of covers. A row that scrolls left and right by default,
//! or a column that scrolls up and down by a setting. gpui has no real 3D,
//! so the turn is faked:
//! off-center covers scale down, compress horizontally (ObjectFit::Fill),
//! and darken with distance, which gives the depth without a perspective
//! transform the toolkit can't do. It shares the album grid's whole model,
//! one entry per album in the library's canonical order, textures through
//! the shared artwork service, the same search, follow-playing, dim, and
//! play rules; the difference is shape. Per the workspace rule, a browsing
//! surface is a panel of its own, never a library view mode.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    canvas, div, hsla, img, point, prelude::*, px, relative, size, svg, Along, AnyElement, App,
    Axis, Bounds, BoxShadow, Context, Div, Entity, EventEmitter, FocusHandle, Focusable,
    ImageSource, MouseButton, MouseDownEvent, MouseUpEvent, ObjectFit, Pixels, RenderImage,
    ScrollWheelEvent, SharedString, Size, Subscription, WeakEntity, Window,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Side};
use image::Frame;
use rox_core::QUEUE_CAP;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::TrackKey;
use rox_panel_kit::config::{default_true, is_zero};
use rox_panel_kit::wall::{default_dim, TILE_DIM_MAX};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::discs::{self, DiscCache, DiscShape, DiscStyle};
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

/// The tile rounding knob's ceiling, in percent of circular: 100 rounds a
/// square cover all the way into a circle.
const TILE_ROUNDING_MAX: f32 = 100.;

/// Covers drawn to each side of the centered one. Past this they would be
/// off the panel edge or too small to read, so the carousel stops there.
const VIS: i64 = 5;

/// How much each step out from center shrinks a cover, multiplied per unit
/// of distance, floored at [`MIN_SCALE`].
const SHRINK: f32 = 0.86;
const MIN_SCALE: f32 = 0.5;

/// The first flank's center, in units of the hero's edge: under the hero's
/// half so the neighbor tucks behind it.
const SHIFT0: f32 = 0.56;
/// Each further cover's step past the first, same units.
const STEP: f32 = 0.30;

/// A turned cover's width as a fraction of its height: the horizontal
/// squash that fakes the rotation away from the viewer.
const TURN_EDGE: f32 = 0.4;

/// How fast covers fade with distance from center, and the floor they hold.
const FADE: f32 = 0.16;
const MIN_OP: f32 = 0.2;

/// The label strip's height under the hero, reserved out of the panel so
/// the covers sit above it.
const LABEL_H: f32 = 40.;

/// The reflection floor: how far a cover's mirror reaches past its lower
/// edge, as a fraction of the cover, and how bright the mirror starts
/// before it fades to nothing.
const REFL: f32 = 0.32;
const REFL_OP: f32 = 0.45;
/// The seam between a cover and its mirror, in px.
const REFL_GAP: f32 = 2.;

/// The perspective turn: how far a flank cover rotates about its cross
/// axis, in radians, and the projection's focal length in hero edges.
/// The tilt reaches its full angle one step out, like the squash it
/// replaces.
const TILT: f32 = 0.96;
const FOCAL: f32 = 2.8;

/// Wheel travel, in px, that advances the carousel by one cover.
const WHEEL_STEP: f32 = 40.;

/// The carousel size before a first paint has measured the panel.
const FALLBACK_W: f32 = 600.;
const FALLBACK_H: f32 = 320.;

/// The art panel's per-view config: what a saved layout restores, and what
/// the settings window edits.
#[derive(Clone, Serialize, Deserialize)]
pub struct ArtConfig {
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
    /// Whether this shelf filters by its own query or follows the shared
    /// app-wide one. Shared by default.
    #[serde(default)]
    pub query_source: QuerySource,
    /// Stack the shelf as a column that scrolls up and down; the default is
    /// a row that scrolls left and right.
    #[serde(default)]
    pub vertical: bool,
    /// Bring the playing album to the center when the track changes.
    #[serde(default)]
    pub follow_playing: bool,
    /// After the shelf sits untouched for a spell, glide the playing album
    /// back to the center on its own. Off by default; a browse surface only
    /// chases the player once you ask it to.
    #[serde(default)]
    pub resume_playing: bool,
    /// Glide there instead of jumping.
    #[serde(default)]
    pub smooth_follow: bool,
    /// While a track plays, fade every cover but the playing album's;
    /// hovering lights a cover back up.
    #[serde(default)]
    pub dim_playing: bool,
    /// The same focus effect in color: drain every cover but the playing
    /// album's to grayscale while a track plays. Stacks with `dim_playing`
    /// or stands on its own.
    #[serde(default)]
    pub desaturate_playing: bool,
    /// Keep the dim and desaturate effects on all the time, not only while a
    /// track plays: every cover but the one under the pointer recedes,
    /// playing or not.
    #[serde(default)]
    pub dim_always: bool,
    /// How far the dimmed covers fade, in percent of fully hidden.
    #[serde(default = "default_dim")]
    pub dim: f32,
    /// Each cover's corner rounding, in percent of circular: zero keeps the
    /// covers square, 100 rounds each into a circle.
    #[serde(default)]
    pub rounding: f32,
    /// Mirror each cover past its lower edge, fading into the background:
    /// the shelf's glass floor. On by default; it's the look the carousel
    /// is for.
    #[serde(default = "default_true")]
    pub reflection: bool,
    /// A soft shadow under every cover.
    #[serde(default)]
    pub shadow: bool,
    /// An accent-tinted pool of light behind the centered cover. The accent
    /// follows the art tint, so with the tint on the glow takes the playing
    /// album's color by itself.
    #[serde(default)]
    pub glow: bool,
    /// Dress every cover as a disc: off, CD, or vinyl, the cover panel's
    /// styles on the whole rack. The rounding knob stands down while a
    /// style is on; a disc is already round.
    #[serde(default)]
    pub disc_style: DiscStyle,
    /// Turn the side covers in real 3D: a projected keystone through the
    /// sprite pipeline instead of the flat squash. On by default; off is
    /// the old squash-and-scrim look, which also keeps art rounding.
    #[serde(default = "default_true")]
    pub perspective: bool,
    /// The album at the center when the layout was saved, so a relaunch
    /// reopens the shelf where it was left. A cell index.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub center: usize,
}

impl Default for ArtConfig {
    fn default() -> Self {
        ArtConfig {
            chrome: PanelChrome::default(),
            query: String::new(),
            search: false,
            query_source: QuerySource::default(),
            vertical: false,
            follow_playing: false,
            resume_playing: false,
            smooth_follow: false,
            dim_playing: false,
            desaturate_playing: false,
            dim_always: false,
            dim: default_dim(),
            rounding: 0.,
            reflection: true,
            shadow: false,
            glow: false,
            disc_style: DiscStyle::Off,
            perspective: true,
            center: 0,
        }
    }
}

/// One album's run in the current view: where it starts, how many tracks it
/// spans, and the first track's path once a paint resolved it (the inner
/// None is a track the store no longer knows).
struct Cell {
    start: usize,
    len: u32,
    art: Option<Option<PathBuf>>,
    /// The cover's current opacity under the dim mode, easing toward its
    /// target every frame. None until the cover's first paint, which lands
    /// at the target directly: only changes fade.
    dim: Option<f32>,
}

/// A quad's axis-aligned box: what the interactive div spans while the
/// canvas inside paints the keystone.
fn quad_aabb(quad: &[[f32; 2]; 4]) -> (f32, f32, f32, f32) {
    let (mut min_x, mut min_y) = (quad[0][0], quad[0][1]);
    let (mut max_x, mut max_y) = (min_x, min_y);
    for [x, y] in quad {
        min_x = min_x.min(*x);
        min_y = min_y.min(*y);
        max_x = max_x.max(*x);
        max_y = max_y.max(*y);
    }
    (min_x, min_y, max_x - min_x, max_y - min_y)
}

/// The centered square of an image as the fractional source rect
/// `paint_image_quad` crops to: what `ObjectFit::Cover` shows in the
/// shelf's square boxes, since thumbs cap their longest side rather than
/// baking square.
fn square_source(data: &RenderImage) -> Bounds<f32> {
    let size_px = data.size(0);
    let (iw, ih) = (size_px.width.0 as f32, size_px.height.0 as f32);
    if iw <= 0. || ih <= 0. {
        return Bounds {
            origin: point(0., 0.),
            size: size(1., 1.),
        };
    }
    if iw > ih {
        Bounds {
            origin: point((1. - ih / iw) / 2., 0.),
            size: size(ih / iw, 1.),
        }
    } else {
        Bounds {
            origin: point(0., (1. - iw / ih) / 2.),
            size: size(1., iw / ih),
        }
    }
}

/// Where a cover paints and how, shared by the cover and its mirror.
struct Placement {
    left: f32,
    top: f32,
    w: f32,
    h: f32,
    /// The distance fade, before the dim mode multiplies in.
    fade: f32,
    /// How far the cover has turned away: 0 at the hero, 1 a full step out.
    turn: f32,
}

pub struct ArtPanel {
    state: AppState,
    config: ArtConfig,
    /// The rows the cells index into: the canonical order while the query
    /// is empty, otherwise the search hits re-ordered canonically so an
    /// album's tracks stay one contiguous run.
    view: Arc<Vec<u32>>,
    /// The albums of the current view, one cover each, rebuilt on library
    /// updates and query changes.
    cells: Vec<Cell>,
    /// The cells whose dim is still easing toward its target, so the fade
    /// steps just these plus the visible window instead of scanning every
    /// cover in a big library each frame.
    dimming: HashSet<usize>,
    /// The baked disc faces while a disc style is on, keyed by art path
    /// and filled off-thread as covers come into view.
    discs: DiscCache,
    /// The query editor, the shared search box; `config.query` mirrors its
    /// value via change events.
    search: Entity<SearchBox>,
    /// The centered album published on the shared selection. A set of one:
    /// the carousel centers a single album, but the context menu and
    /// publish path stay the grid's, which act on a set.
    selected: HashSet<usize>,
    /// The cover under the pointer, which lifts out of the dim.
    hovered: Option<usize>,
    /// The animated center: the album at the middle of the shelf, a
    /// fractional index while a move is in flight.
    pos: f32,
    /// Where `pos` is easing to: a whole cell index once settled.
    goal: f32,
    /// True while a free scrub (drag or its coast) owns `pos`; the release
    /// snaps `goal` to the nearest cover. Clears once the ease takes over.
    coasting: bool,
    /// A pending selection publish from a user move: set by a drag, wheel,
    /// or click, spent on the next settle so a scrub only publishes the
    /// album it lands on, not every one it passes.
    publish_pending: bool,
    /// Wheel travel banked toward the next [`WHEEL_STEP`].
    wheel: f32,
    /// The panel's measured content box, the carousel's frame. The dock
    /// hosts panels cached, so a resize repaints without re-rendering; a
    /// measuring canvas compares against this and notifies on drift.
    size: Size<Pixels>,
    /// The drag-to-scrub state: press anywhere, drag to spin the shelf,
    /// release to coast and snap. A drag past its dead zone swallows the
    /// cover click.
    flick: FlickState,
    /// The last animation tick, the ease's and the coast's dt.
    last_tick: Instant,
    /// The idle-resume clock: stamped on every scroll or press, it wakes
    /// the playing album back to the center once `resume_playing` is on and
    /// the user has stepped away.
    resume_idle: ResumeIdle,
    /// The playing track's path, the change detector for follow-playing.
    playing_key: Option<TrackKey>,
    /// The playing album's cell in the current view, kept fresh by
    /// `sync_playing` and `rebuild` so per-frame dimming never rescans.
    playing_ix: Option<usize>,
    /// Whether audio is moving right now; pause lifts the dim.
    playing: bool,
    /// The centered cell the last settle published, so a settle only
    /// republishes when the album at the middle actually changed.
    centered: Option<usize>,
    /// The cover rounding slider's scrub strip, for the settings window.
    rounding_scrub: ScrubState,
    /// The dim amount slider's scrub strip, same window.
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
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _library_changed: Subscription,
    _thumbs_changed: Subscription,
    _search_events: Subscription,
    _query_changed: Subscription,
    _selection_changed: Subscription,
    _player_changed: Subscription,
}

impl ArtPanel {
    pub fn new(
        state: AppState,
        config: ArtConfig,
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
                // re-center on the playing album the same way.
                if this.config.follow_playing {
                    this.follow_playing(cx);
                }
            },
        );
        // Landing thumbnails notify the service; repaint so covers fill in.
        let _thumbs_changed = cx.observe(&state.thumbs, |_, _, cx| cx.notify());
        // A shelf restored as global opens showing the shared query; an
        // own-query one shows its own.
        let initial = match config.query_source {
            QuerySource::Global => state.query.read(cx).text().to_string(),
            QuerySource::Local | QuerySource::Selection => config.query.clone(),
        };
        let search = cx.new(|cx| SearchBox::new("Search", &initial, window, cx).small());
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        // Follow the shared query while global: rebuild the shelf and reset
        // the box to it on the next render.
        let _query_changed = cx.subscribe(
            &state.query,
            |this: &mut Self, _, _: &SharedQueryEvent, cx| {
                this.on_shared_query_changed(cx);
            },
        );
        // An art shelf restored as selection-following opens on whatever is
        // picked now, rather than blank until the next pick.
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
        // Follow-playing owns the center on launch, so it skips the saved
        // one; otherwise the shelf reopens where it was left.
        let start = if config.follow_playing {
            0
        } else {
            config.center
        } as f32;
        let mut this = ArtPanel {
            state,
            config,
            view: Arc::new(Vec::new()),
            cells: Vec::new(),
            dimming: HashSet::new(),
            discs: DiscCache::default(),
            search,
            selected: HashSet::new(),
            hovered: None,
            pos: start,
            goal: start,
            coasting: false,
            publish_pending: false,
            wheel: 0.,
            size: Size::default(),
            flick: FlickState::default(),
            last_tick: Instant::now(),
            resume_idle: ResumeIdle::default(),
            playing_key: None,
            playing_ix: None,
            playing: false,
            // Suppress the launch settle's publish: a restore reopens a
            // position, it does not reach out and reselect.
            centered: Some(start as usize),
            rounding_scrub: ScrubState::default(),
            dim_scrub: ScrubState::default(),
            value_edit: panel::ValueEdit::default(),
            error: None,
            resync_box: false,
            selection_ids,
            focus: cx.focus_handle(),
            tab_panel: None,
            _library_changed,
            _thumbs_changed,
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

    /// Follow the player: on a track change, center the album it lives in,
    /// and keep the dim mode's facts fresh. The compares keep the per-tick
    /// observer cheap, the player notifies every pump.
    fn sync_playing(&mut self, cx: &mut Context<Self>) {
        let (playing, path) = {
            let player = self.state.player.read(cx);
            (player.is_playing(), player.now_playing().map(|now| now.key))
        };
        if playing != self.playing {
            // Pause lifts the dim, resuming drops it back; render steps the
            // fade, this kicks it off.
            self.playing = playing;
            cx.notify();
        }
        if path == self.playing_key {
            return;
        }
        self.playing_key = path;
        self.playing_ix = self.playing_cell(cx);
        if self.config.follow_playing {
            self.follow_playing(cx);
        }
        cx.notify();
    }

    /// The playing track's album in the current view, when it holds one.
    fn playing_cell(&self, cx: &App) -> Option<usize> {
        let key = self.playing_key.as_ref()?;
        let library = self.state.library.read(cx);
        let id = library.id_for_key(key)?;
        let projection = library.projection()?;
        let view_ix = self
            .view
            .iter()
            .position(|&row| projection.db_id[row as usize] == id)?;
        // Cells are contiguous runs over the view; the last one starting at
        // or before the hit holds it.
        Some(
            self.cells
                .partition_point(|cell| cell.start <= view_ix)
                .saturating_sub(1),
        )
    }

    /// Center the playing track's album: a glide when smooth is on, a jump
    /// otherwise. The automatic follow never touches the selection.
    fn follow_playing(&mut self, cx: &mut Context<Self>) {
        let Some(cell_ix) = self.playing_ix else {
            return;
        };
        self.goal = cell_ix as f32;
        self.coasting = false;
        if !self.config.smooth_follow {
            self.pos = self.goal;
            self.centered = Some(cell_ix);
        }
        cx.notify();
    }

    /// The menu's jump: select the playing track's album and center it with
    /// the panel's configured motion. Unlike the automatic follow, this
    /// deliberate move publishes the selection.
    fn jump_to_playing(&mut self, cx: &mut Context<Self>) {
        let Some(cell_ix) = self.playing_ix else {
            return;
        };
        self.select_only(cell_ix, cx);
        self.follow_playing(cx);
    }

    /// A scroll, drag, or press: restart the idle clock and arm a wake, so
    /// the shelf drifts the playing album back to the center once the user
    /// steps away. A no-op unless the resume behavior is on, so an off
    /// panel spends nothing per gesture.
    fn touch_resume(&mut self, cx: &mut Context<Self>) {
        if self.config.resume_playing {
            self.resume_idle.touch(cx, Self::resume_to_playing);
        }
    }

    /// The idle wake's landing: glide the playing album back to the center,
    /// so long as the resume is still on. The clock only fires this once the
    /// shelf has sat untouched a full window, a gesture in between having
    /// pushed it out, so no extra idle check is needed here.
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

    /// Aim the carousel at a cell with the ease, from a user move. Marks the
    /// settle to publish so the album it lands on reaches the selection.
    fn navigate(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.goal = ix as f32;
        self.coasting = false;
        self.publish_pending = true;
        cx.notify();
    }

    /// Flip the scroll axis, from the context menu or the settings toggle.
    /// Every cover re-sizes off the cross axis, so drop the measured frame
    /// and let the next paint measure it fresh.
    fn set_orientation(&mut self, vertical: bool, cx: &mut Context<Self>) {
        if self.config.vertical == vertical {
            return;
        }
        self.config.vertical = vertical;
        self.size = Size::default();
        cx.notify();
    }

    /// Recompute the view and its album runs: the canonical order, cut to
    /// the query's hits when one is set. Search hits come back in
    /// projection row order, so they filter the canonical order rather than
    /// getting walked directly - otherwise an album's scattered rows would
    /// split into duplicate covers. Breaks on the album artist, not the
    /// track artist, so a compilation stays one cover.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.cells.clear();
        self.dimming.clear();
        self.selected.clear();
        // The settle dedupe keys on a cell index, and the rebuild may have
        // just reordered or refiltered the cells under it - the same index
        // can now be a different album, and a settle there must publish.
        self.centered = None;
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
        let has_projection = self.state.library.read(cx).projection().is_some();
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
        // A shorter view (a query) can leave the center past the end. Only
        // re-clamp once the projection is loaded, though: on a cold start it
        // has not arrived, so there are no cells yet and clamping here would
        // pin the restored center to 0 before the shelf ever builds.
        if has_projection {
            let max = self.max_index();
            self.pos = self.pos.clamp(0., max);
            self.goal = self.goal.clamp(0., max);
        }
        self.playing_ix = self.playing_cell(cx);
        cx.notify();
    }

    /// The last valid center index as a float, zero for an empty view.
    fn max_index(&self) -> f32 {
        self.cells.len().saturating_sub(1) as f32
    }

    /// Map the shared box's events onto the shelf: a changed query rebuilds
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

    /// The artist a cover filters by: its first track's, the shelf's
    /// stand-in for the album's artist. None off the end of the cells.
    fn cell_artist(&self, ix: usize, cx: &App) -> Option<String> {
        let cell = self.cells.get(ix)?;
        let row = *self.view.get(cell.start)?;
        let library = self.state.library.read(cx);
        let projection = library.projection()?;
        Some(projection.resolve(row).artist.to_string())
    }

    /// The path a cover loads by: the album's first track, resolved through
    /// the store once, on the cover's first paint.
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

    /// Make one album the selection and publish it. The carousel centers a
    /// single album, so a move replaces the set rather than growing it.
    fn select_only(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = HashSet::from([ix]);
        self.centered = Some(ix);
        self.publish_selection(cx);
        cx.notify();
    }

    /// Resolve the selected album to db ids in view order and publish them
    /// on the shared selection.
    fn publish_selection(&mut self, cx: &mut Context<Self>) {
        let mut ixs: Vec<usize> = self.selected.iter().copied().collect();
        ixs.sort_unstable();
        let ids: Vec<i64> = ixs.iter().flat_map(|&ix| self.ids_for(ix, cx)).collect();
        let source = cx.entity_id();
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, source, cx));
    }

    /// Queue the album on the shared player.
    fn play(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.play_many(vec![ix], cx);
    }

    /// Queue several albums on the shared player, in view order under the
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

    /// The axis the shelf stacks and scrolls along.
    fn axis(&self) -> Axis {
        if self.config.vertical {
            Axis::Vertical
        } else {
            Axis::Horizontal
        }
    }

    /// The panel's measured content box, or the fallback until the first
    /// paint measures it.
    fn frame(&self) -> (f32, f32) {
        let w = f32::from(self.size.width);
        let h = f32::from(self.size.height);
        if w <= 0. || h <= 0. {
            (FALLBACK_W, FALLBACK_H)
        } else {
            (w, h)
        }
    }

    /// The hero cover's edge in px: as big as the cross axis allows, capped
    /// so the panel still fits it along the scroll axis. The label always
    /// takes its strip off the panel's bottom, so a row loses it from the
    /// height and a column from the length.
    fn hero_side(&self) -> f32 {
        let (w, h) = self.frame();
        let avail_h = h - LABEL_H;
        // The mirrors take their strips out of the cross axis: a row has
        // one floor below, a column mirrors both side edges, so covers
        // and reflections fit together either way.
        let (floor, sides) = if self.config.reflection {
            (1.0 + REFL, 1.0 + 2.0 * REFL)
        } else {
            (1.0, 1.0)
        };
        match self.axis() {
            // A row: covers as tall as the band, capped by the width.
            Axis::Horizontal => (avail_h * 0.9 / floor).min(w * 0.42),
            // A column: covers as wide as the panel, capped by its length.
            Axis::Vertical => (w * 0.86 / sides).min(avail_h * 0.42),
        }
        .max(48.)
    }

    /// Px of drag along the scroll axis per cover of travel, the coast and
    /// pointer mapping.
    fn step_px(&self) -> f32 {
        (self.hero_side() * STEP).max(1.)
    }

    /// Whether cover `ix` sits in the receded set: the covers the focus
    /// effects push back. The hovered cover and the playing album are always
    /// exempt. Always mode pushes back every other cover; otherwise only the
    /// rest while audio moves.
    fn receded(&self, ix: usize) -> bool {
        if self.hovered == Some(ix) || self.playing_ix == Some(ix) {
            return false;
        }
        self.config.dim_always || self.playing
    }

    /// A cover's resting opacity under the dim mode: the configured floor
    /// for a receded cover, full otherwise.
    fn dim_target(&self, ix: usize) -> f32 {
        if self.config.dim_playing && self.receded(ix) {
            1.0 - self.config.dim / TILE_DIM_MAX
        } else {
            1.0
        }
    }

    /// Whether a cover paints grayscale under the desaturate mode.
    fn desaturated(&self, ix: usize) -> bool {
        self.config.desaturate_playing && self.receded(ix)
    }

    /// The disc face for a cover, baked from its thumb off-thread: a hit
    /// comes back at once, a miss claims one bake and comes back None
    /// until it lands. The bytes are the thumb's own, so the face is as
    /// sharp as the flat cover it replaces and the bake stays a few
    /// milliseconds.
    fn disc_of(
        &mut self,
        path: PathBuf,
        image: Arc<gpui::Image>,
        cx: &mut Context<Self>,
    ) -> Option<Arc<RenderImage>> {
        if let Some(disc) = self.discs.ready(&path) {
            return Some(disc);
        }
        let shape = match self.config.disc_style {
            DiscStyle::Cd => DiscShape::Cd,
            DiscStyle::Vinyl => DiscShape::Vinyl,
            DiscStyle::Off => return None,
        };
        if !self.discs.begin(&path) {
            return None;
        }
        cx.spawn(async move |this, cx| {
            let baked = cx
                .background_executor()
                .spawn(async move {
                    discs::bake_disc(&image.bytes, shape)
                        .map(|disc| Arc::new(RenderImage::new(vec![Frame::new(disc)])))
                })
                .await;
            this.update(cx, |this, cx| {
                this.discs.finish(&path, baked);
                cx.notify();
            })
            .ok();
        })
        .detach();
        None
    }

    /// The canvas that paints a face onto a projected quad through
    /// `paint_image_quad`: a disc bake paints directly, a thumb resolves
    /// through the img element's own asset cache and center-crops square,
    /// the crop `ObjectFit::Cover` would take. The quad arrives relative
    /// to the canvas' top-left; the paint hook re-anchors it to wherever
    /// the div actually lands.
    fn quad_canvas(
        quad: [[f32; 2]; 4],
        image: Arc<gpui::Image>,
        bake: Option<Arc<RenderImage>>,
        grayscale: bool,
        fade: [f32; 4],
        wash: gpui::Hsla,
        flip: Option<Axis>,
    ) -> AnyElement {
        canvas(
            |_, _, _| (),
            move |bounds: Bounds<Pixels>, _, window, cx| {
                let full = Bounds {
                    origin: point(0., 0.),
                    size: size(1., 1.),
                };
                let data = match &bake {
                    Some(bake) => Some((bake.clone(), full)),
                    None => ImageSource::from(image.clone())
                        .use_data(None, window, cx)
                        .and_then(|result| result.ok())
                        .map(|data| {
                            let source = square_source(&data);
                            (data, source)
                        }),
                };
                let Some((data, mut source)) = data else {
                    return;
                };
                // A mirror samples backwards instead of inverting its
                // corners, so the vertex map stays orientation-true. A
                // row's floor flips the rows, a column's side mirrors
                // flip the columns.
                match flip {
                    Some(Axis::Horizontal) => {
                        source.origin.y += source.size.height;
                        source.size.height = -source.size.height;
                    }
                    Some(Axis::Vertical) => {
                        source.origin.x += source.size.width;
                        source.size.width = -source.size.width;
                    }
                    None => {}
                }
                let corners = quad
                    .map(|[x, y]| gpui::point(bounds.origin.x + px(x), bounds.origin.y + px(y)));
                let _ = window.paint_image_quad(corners, source, data, 0, grayscale, fade, wash);
            },
        )
        .absolute()
        .size_full()
        .into_any_element()
    }

    /// Pick the disc dress-up. The bakes are per style, so flipping it
    /// throws the cache and the shelf re-bakes as covers come into view.
    fn set_disc_style(&mut self, style: DiscStyle, cx: &mut Context<Self>) {
        if self.config.disc_style != style {
            self.config.disc_style = style;
            self.discs.clear();
        }
        cx.notify();
    }

    /// A cover's box at distance `d` from the center: position, size, the
    /// distance fade before the dim mode multiplies in, and how far the
    /// cover has turned away. The cover and its reflection read the same
    /// numbers, which is what keeps the mirror under its cover through
    /// every scrub frame.
    fn placement(&self, d: f32, hero: f32, cx_px: f32, cy_px: f32) -> Placement {
        let a = d.abs();
        let scale = SHRINK.powf(a).max(MIN_SCALE);
        // The turn ramps in over the first step out and holds; a turned
        // cover is compressed to TURN_EDGE of its face along the scroll axis.
        let turn = a.clamp(0., 1.);
        let turn_factor = 1.0 - turn * (1.0 - TURN_EDGE);
        // The face across the scroll axis keeps the scale; the one along it
        // squashes with the turn. A row squashes the width, a column the
        // height, so either way the cover looks turned away from you.
        let cross_size = hero * scale;
        let along_size = cross_size * turn_factor;
        let off = Self::offset_units(d) * hero;
        let (cover_x, cover_y, w, h) = match self.axis() {
            Axis::Horizontal => (cx_px + off, cy_px, along_size, cross_size),
            Axis::Vertical => (cx_px, cy_px + off, cross_size, along_size),
        };
        Placement {
            left: cover_x - w / 2.0,
            top: cover_y - h / 2.0,
            w,
            h,
            fade: (1.0 - a * FADE).max(MIN_OP),
            turn,
        }
    }

    /// A cover's projected quad at distance `d`: corners clockwise from
    /// the texture's top-left, in shelf coordinates. The cover rotates
    /// about its cross axis through its center, inner edge toward the
    /// viewer, and projects with a focal length a few covers deep - the
    /// keystone the squash was faking. The shrink keeps the near edge
    /// under the hero's height, so the flanks never outgrow the band.
    fn quad(&self, d: f32, hero: f32, cx_px: f32, cy_px: f32) -> [[f32; 2]; 4] {
        let a = d.abs();
        let scale = SHRINK.powf(a).max(MIN_SCALE);
        let half = hero * scale / 2.0;
        let theta = TILT * a.clamp(0., 1.) * d.signum();
        let (sin, cos) = theta.sin_cos();
        let focal = hero * FOCAL;
        let off = Self::offset_units(d) * hero;
        // An edge at offset `u` along the scroll axis sits at depth
        // u * sin, so the inner edge swings toward the viewer and grows
        // while the outer one recedes.
        let edge = |u: f32| {
            let s = focal / (focal + u * sin);
            // The projection can push a flank's near edge a hair past
            // the hero's band; the cap keeps every cover inside it, so
            // the floor seams hold one clean line under the shelf.
            let s = s.min(hero / 2.0 / half);
            (u * cos * s, half * s)
        };
        let (near, near_half) = edge(-half);
        let (far, far_half) = edge(half);
        match self.axis() {
            Axis::Horizontal => {
                let cx0 = cx_px + off;
                [
                    [cx0 + near, cy_px - near_half],
                    [cx0 + far, cy_px - far_half],
                    [cx0 + far, cy_px + far_half],
                    [cx0 + near, cy_px + near_half],
                ]
            }
            Axis::Vertical => {
                let cy0 = cy_px + off;
                [
                    [cx_px - near_half, cy0 + near],
                    [cx_px + near_half, cy0 + near],
                    [cx_px + far_half, cy0 + far],
                    [cx_px - far_half, cy0 + far],
                ]
            }
        }
    }

    /// A cover's center offset from the hero, in units of the hero's edge:
    /// the first neighbor tucks under the hero's half, each further one
    /// steps out past it.
    fn offset_units(d: f32) -> f32 {
        let s = d.signum();
        let a = d.abs();
        if a <= 1.0 {
            s * SHIFT0 * a
        } else {
            s * (SHIFT0 + STEP * (a - 1.0))
        }
    }

    /// One cover in the carousel: placed absolutely by its distance `d` from
    /// the center, scaled and turned and dimmed by it. Pending and missing
    /// art wear the same quiet placeholder, so a landing cover fills in
    /// without a flash.
    fn cover(
        &mut self,
        ix: usize,
        d: f32,
        hero: f32,
        cx_px: f32,
        cy_px: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The first paint lands at the dim target directly; from then on the
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
        let Placement {
            left: flat_left,
            top: flat_top,
            w: flat_w,
            h: flat_h,
            fade,
            turn,
        } = self.placement(d, hero, cx_px, cy_px);
        let opacity = fade * dim;
        let disc_on = self.config.disc_style != DiscStyle::Off;
        let persp = self.config.perspective;
        // With perspective on the div spans the keystone's box and the
        // canvas inside paints the real shape; off, the flat squash rect.
        let quad = persp.then(|| self.quad(d, hero, cx_px, cy_px));
        let (left, top, w, h) = match &quad {
            Some(quad) => quad_aabb(quad),
            None => (flat_left, flat_top, flat_w, flat_h),
        };
        // A keystone can't clip rounded, so perspective paints square; a
        // disc pins the radius to the pill hugging the bake's circle, so
        // the ring and the shadow follow the disc's outline; otherwise
        // the rounding knob has its say.
        let radius = if persp {
            px(0.)
        } else if disc_on {
            px(w.min(h) / 2.)
        } else {
            px(w.min(h) * (self.config.rounding / 200.))
        };
        let is_hero = d.abs() < 0.5;

        let path = self.art_path(ix, cx);
        let thumb = match &path {
            Some(path) => self
                .state
                .thumbs
                .update(cx, |thumbs, cx| thumbs.get(path, cx)),
            None => Thumb::Missing,
        };
        // The disc face once its bake lands; until then the flat thumb
        // stands in behind the pill crop, so scrubbing never flashes.
        let baked = match (&thumb, &path, disc_on) {
            (Thumb::Ready(image), Some(path), true) => {
                self.disc_of(path.clone(), image.clone(), cx)
            }
            _ => None,
        };
        // The hero keeps its aspect (Cover crops to the square); turned
        // covers Fill, which stretches the whole art into the compressed
        // box, the foreshortening along the scroll axis that reads as a turn.
        let fit = if is_hero {
            ObjectFit::Cover
        } else {
            ObjectFit::Fill
        };
        let desaturated = self.desaturated(ix);
        let disc_shown = baked.is_some();
        let quad_painted = quad.is_some() && matches!(thumb, Thumb::Ready(_));
        let content: AnyElement = match (&quad, thumb, baked) {
            // The keystone: the face rides the canvas onto the projected
            // quad, and the wash bakes the turn scrim into the sprite,
            // shaped to the quad the way an overlay div can't be.
            (Some(quad), Thumb::Ready(image), bake) => {
                let wash = palette::alpha(palette::bg_root(), (turn * 90.) as u8).into();
                let rel = quad.map(|[x, y]| [x - left, y - top]);
                Self::quad_canvas(rel, image, bake, desaturated, [1.; 4], wash, None)
            }
            // The bake is square with the disc touching its edges and
            // carrying its own alpha, so the fill stretch is what turns it
            // with the box and nothing needs clipping into shape.
            (None, Thumb::Ready(_), Some(disc)) => img(disc)
                .size_full()
                .object_fit(ObjectFit::Fill)
                .grayscale(desaturated)
                .into_any_element(),
            (None, Thumb::Ready(image), None) => img(image)
                .size_full()
                // Cover only crops if something masks it; gpui paints the
                // overrun otherwise, and the hero would spill onto the
                // covers turned beside it.
                .overflow_hidden()
                .object_fit(fit)
                .grayscale(desaturated)
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
            .absolute()
            .left(px(left))
            .top(px(top))
            .w(px(w))
            .h(px(h))
            .overflow_hidden()
            .rounded(radius)
            // A landed disc face carries its own alpha, and a keystone
            // leaves its box's corners bare; backdrop behind either would
            // show where it shouldn't.
            .when(!disc_shown && !quad_painted, |el| {
                el.bg(palette::bg_elevated())
            })
            .opacity(opacity)
            // A contact shadow scaled to the cover, so far covers cast
            // smaller ones and the shelf keeps one light.
            .when(self.config.shadow, |el| {
                el.shadow(vec![BoxShadow {
                    color: hsla(0., 0., 0., 0.35),
                    offset: point(px(0.), px(h * 0.05)),
                    blur_radius: px(h * 0.10),
                    spread_radius: px(0.),
                }])
            })
            .cursor_pointer()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let target = hovered.then_some(ix);
                if this.hovered != target && (this.hovered == Some(ix) || *hovered) {
                    this.hovered = target;
                    cx.notify();
                }
            }))
            // A press might start a scrub; the click acts on release, and a
            // scrub that traveled is not a click.
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                    if this.flick.scrolled() {
                        return;
                    }
                    this.focus.focus(window);
                    if event.click_count > 1 {
                        this.play(ix, cx);
                        this.navigate(ix, cx);
                    } else {
                        // Center it and select it; the settle republishes,
                        // this makes the click feel immediate.
                        this.select_only(ix, cx);
                        this.navigate(ix, cx);
                    }
                }),
            )
            .child(content)
            // A dark scrim deepens with the turn, so an angled cover reads as
            // lit from the front like the hero. Perspective covers carry the
            // same shading in their wash instead.
            .when(!quad_painted && turn > 0.02, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(radius)
                        .bg(palette::alpha(palette::bg_root(), (turn * 90.) as u8)),
                )
            })
            .when(is_hero && self.selected.contains(&ix), |d| {
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

    /// A cover's mirrors: the same face painted through the quad
    /// pipeline, flipped past the cover's edge and fading to nothing by
    /// [`REFL`] of the way out - a true alpha fade, so the glow and
    /// whatever else lies underneath shows through. A row gets the one
    /// floor below; a column has no floor to stand on, so it mirrors both
    /// side edges and stays symmetric. With perspective on the mirrors
    /// keystone with their cover, each column reflected past its own
    /// edge, which is exactly the 3D mirror. Only real art reflects; a
    /// placeholder's floor stays bare so a landing cover's mirror fills
    /// in with it.
    fn reflection(
        &mut self,
        ix: usize,
        d: f32,
        hero: f32,
        cx_px: f32,
        cy_px: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let path = self.art_path(ix, cx)?;
        let thumb = self
            .state
            .thumbs
            .update(cx, |thumbs, cx| thumbs.get(&path, cx));
        let Thumb::Ready(image) = thumb else {
            return None;
        };
        let Placement {
            left,
            top,
            w,
            h,
            fade,
            turn: _,
        } = self.placement(d, hero, cx_px, cy_px);
        // The dim as the cover painted it; before a first paint the target
        // stands in, read-only, so the mirror never races the fade.
        let dim = self
            .cells
            .get(ix)
            .and_then(|cell| cell.dim)
            .unwrap_or_else(|| self.dim_target(ix));
        // The mirror shows what its cover shows: the disc face once that
        // bake has landed (read-only here, the cover above claims the
        // bakes), the flat thumb until then.
        let disc_on = self.config.disc_style != DiscStyle::Off;
        let baked = if disc_on {
            self.discs.ready(&path)
        } else {
            None
        };
        // The cover's quad, or its flat rect worn as one: either way the
        // mirror is the same paint call, corners and fade doing the work
        // the strip-and-scrim used to.
        let quad = if self.config.perspective {
            self.quad(d, hero, cx_px, cy_px)
        } else {
            [
                [left, top],
                [left + w, top],
                [left + w, top + h],
                [left, top + h],
            ]
        };
        // The fade runs 1 at the seam to 0 at REFL of the way out; the
        // far corners sit past zero so the ramp lands there, and the
        // shader clamps the rest of the face to nothing.
        let spent = 1.0 - 1.0 / REFL;
        let desaturated = self.desaturated(ix);
        // Mirrors are see-through, so overlapping ones double-expose
        // instead of occluding. The floor has to tile the way the shelf
        // does: each mirror clips to the slice of its cover the nearer
        // neighbor toward the center leaves visible, and the seams land
        // exactly on the shelf's own occlusion edges.
        let axis = self.axis();
        let (clip_lo, clip_hi) = if d.abs() > 0.5 {
            let occluder = self.quad(d - d.signum(), hero, cx_px, cy_px);
            let (ol, ot, ow, oh) = quad_aabb(&occluder);
            match axis {
                Axis::Horizontal if d > 0. => (ol + ow, f32::INFINITY),
                Axis::Horizontal => (f32::NEG_INFINITY, ol),
                Axis::Vertical if d > 0. => (ot + oh, f32::INFINITY),
                Axis::Vertical => (f32::NEG_INFINITY, ot),
            }
        } else {
            (f32::NEG_INFINITY, f32::INFINITY)
        };
        // One positioned mirror: a masked div over the visible slice, the
        // full quad canvas inside. The quads keep their natural screen
        // orientation - the flip happens in the canvas' texture sampling -
        // so a mirror sprite is the same orientation-true map as any
        // cover, and the mask clips it without touching the mapping.
        let mirror_el = |mirror: [[f32; 2]; 4], fade_corners: [f32; 4]| {
            let (rl, rt, rw, rh) = quad_aabb(&mirror);
            let (mut cl, mut ct, mut cw, mut ch) = (rl, rt, rw, rh);
            match axis {
                Axis::Horizontal => {
                    let left = rl.max(clip_lo);
                    let right = (rl + rw).min(clip_hi);
                    cl = left;
                    cw = right - left;
                }
                Axis::Vertical => {
                    let top = rt.max(clip_lo);
                    let bottom = (rt + rh).min(clip_hi);
                    ct = top;
                    ch = bottom - top;
                }
            }
            if cw <= 0. || ch <= 0. {
                return None;
            }
            let rel = mirror.map(|[x, y]| [x - cl, y - ct]);
            Some(
                div()
                    .absolute()
                    .left(px(cl))
                    .top(px(ct))
                    .w(px(cw))
                    .h(px(ch))
                    .overflow_hidden()
                    .child(Self::quad_canvas(
                        rel,
                        image.clone(),
                        baked.clone(),
                        desaturated,
                        fade_corners,
                        palette::alpha(palette::bg_root(), 0).into(),
                        Some(axis),
                    )),
            )
        };
        let element = match axis {
            // A row has one floor: each column reflects across its own
            // bottom edge, so the mirror meets its cover exactly however
            // the keystone leans. Seam on top, faded far edge below.
            Axis::Horizontal => mirror_el(
                [
                    [quad[3][0], quad[3][1] + REFL_GAP],
                    [quad[2][0], quad[2][1] + REFL_GAP],
                    [quad[2][0], 2. * quad[2][1] - quad[1][1] + REFL_GAP],
                    [quad[3][0], 2. * quad[3][1] - quad[0][1] + REFL_GAP],
                ],
                [1., 1., spent, spent],
            )
            .map(|el| el.into_any_element()),
            // Sideways has no floor, so a column shelf mirrors both side
            // edges and stays symmetric; each row reflects past its own
            // edge, left and right, seams inward.
            Axis::Vertical => {
                let left = mirror_el(
                    [
                        [2. * quad[0][0] - quad[1][0] - REFL_GAP, quad[0][1]],
                        [quad[0][0] - REFL_GAP, quad[0][1]],
                        [quad[3][0] - REFL_GAP, quad[3][1]],
                        [2. * quad[3][0] - quad[2][0] - REFL_GAP, quad[3][1]],
                    ],
                    [spent, 1., 1., spent],
                );
                let right = mirror_el(
                    [
                        [quad[1][0] + REFL_GAP, quad[1][1]],
                        [2. * quad[1][0] - quad[0][0] + REFL_GAP, quad[1][1]],
                        [2. * quad[2][0] - quad[3][0] + REFL_GAP, quad[2][1]],
                        [quad[2][0] + REFL_GAP, quad[2][1]],
                    ],
                    [1., spent, spent, 1.],
                );
                if left.is_none() && right.is_none() {
                    None
                } else {
                    Some(
                        div()
                            .absolute()
                            .inset_0()
                            .when_some(left, |el, mirror| el.child(mirror))
                            .when_some(right, |el, mirror| el.child(mirror))
                            .into_any_element(),
                    )
                }
            }
        };
        let element = element?;
        Some(
            div()
                .absolute()
                .inset_0()
                .opacity(fade * dim * REFL_OP)
                .child(element)
                .into_any_element(),
        )
    }

    /// The accent pool behind the hero: an unpainted circle whose blurred
    /// shadow is the glow, the cheap radial gradient. The accent follows
    /// the art tint, so the glow takes the playing album's color on its
    /// own.
    fn glow(&self, hero: f32, cx_px: f32, cy_px: f32) -> AnyElement {
        let side = hero * 0.9;
        div()
            .absolute()
            .left(px(cx_px - side / 2.))
            .top(px(cy_px - side / 2.))
            .w(px(side))
            .h(px(side))
            .rounded_full()
            .shadow(vec![BoxShadow {
                color: palette::alpha(palette::accent(), 0x40).into(),
                offset: point(px(0.), px(0.)),
                blur_radius: px(hero * 0.45),
                spread_radius: px(hero * 0.08),
            }])
            .into_any_element()
    }

    /// The label under the hero: album over artist, centered.
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
        let has_text = !album.is_empty() || !artist.is_empty();
        div()
            .absolute()
            .left_0()
            .right_0()
            .bottom(px(6.))
            .flex()
            .flex_col()
            .items_center()
            .when(has_text, |d| {
                d.child(
                    // A rounded scrim behind the text keeps it readable over
                    // the covers a column stacks under the hero.
                    div()
                        .max_w(relative(0.9))
                        .px(tokens::SPACE_SM)
                        .py(tokens::SPACE_XS)
                        .rounded(tokens::RADIUS)
                        .bg(palette::alpha(palette::bg_root(), 0xB0))
                        .flex()
                        .flex_col()
                        .items_center()
                        .when(!album.is_empty(), |d| {
                            d.child(
                                div()
                                    .max_w(relative(1.0))
                                    .truncate()
                                    .text_color(palette::text_bright())
                                    .child(album),
                            )
                        })
                        .when(!artist.is_empty(), |d| {
                            d.child(
                                div()
                                    .max_w(relative(1.0))
                                    .truncate()
                                    .text_xs()
                                    .text_color(palette::text_secondary())
                                    .child(artist),
                            )
                        }),
                )
            })
    }

    /// Solo or popped out there is no title bar to host the search, so it
    /// renders as a toolbar row above the shelf instead, the library's move.
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
}

impl PanelSettings for ArtPanel {
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
                        Some("Stack the shelf as a column that scrolls up and down instead of a row"),
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
                        // The box keeps its text; the view snaps to the full
                        // catalog while hidden. Rebuild notifies, the tab
                        // panel repaints the vanishing suffix.
                        this.rebuild(cx);
                        this.refresh_title_bar(cx);
                    },
                    self.config.query_source,
                    |this: &mut Self, source, cx| this.pick_query_source(source, cx),
                    cx,
                ))
                .child(panel::tracking_section(
                    self.config.follow_playing,
                    "Center the playing album whenever the track changes",
                    |this: &mut Self, on, cx| {
                        this.config.follow_playing = on;
                        // Catch up right away instead of waiting for the
                        // next track change.
                        if on {
                            this.follow_playing(cx);
                        }
                        cx.notify();
                    },
                    self.config.resume_playing,
                    "Center the playing album again after you stop browsing",
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
                            Some(
                                "Fade every cover but the playing album's; hovering lights a cover back up",
                            ),
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
                                settings_ui::scalar(
                                    &self.dim_scrub,
                                    &self.value_edit,
                                    self.config.dim,
                                    settings_ui::span(0., TILE_DIM_MAX, "%").hard(),
                                    |this: &mut Self, value, cx| {
                                        this.config.dim = value;
                                        cx.notify();
                                    },
                                    cx,
                                ),
                            ))
                        })
                        .child(setting_row(
                            "Desaturate While Playing",
                            Some("Drain every cover but the playing album's to grayscale; hovering brings a cover's color back"),
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
                                Some("Keep the covers pushed back even when nothing plays; only a hovered cover shows in full"),
                                toggle(
                                    self.config.dim_always,
                                    |this: &mut Self, on, cx| {
                                        this.config.dim_always = on;
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

    /// The shelf's own appearance row on the shared page: the covers'
    /// rounding, a look knob that lives on the config rather than the theme
    /// because it shapes the covers, not the panel frame.
    fn appearance(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rounding = self.config.rounding;
        Some(
            settings_ui::section(
                "Covers",
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(setting_row(
                        "Perspective",
                        Some("Turn the side covers in real 3D instead of the flat squash"),
                        toggle(
                            self.config.perspective,
                            |this: &mut Self, on, cx| {
                                this.config.perspective = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        "Disc Style",
                        Some("Dress every cover as a CD or as a vinyl record's label"),
                        panel::choices(
                            &discs::DISC_STYLES,
                            self.config.disc_style,
                            |this: &mut Self, style, cx| this.set_disc_style(style, cx),
                            cx,
                        ),
                    ))
                    // A disc is already round and a keystone can't clip
                    // rounded, so the knob only shows while the covers
                    // paint flat and square.
                    .when(
                        self.config.disc_style == DiscStyle::Off && !self.config.perspective,
                        |page| {
                        page.child(setting_row(
                            "Art Rounding",
                            Some("Round each cover's corners; 100% is a circle"),
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
                        ))
                    })
                    .child(setting_row(
                        "Reflections",
                        Some("Mirror each cover into the floor below the shelf"),
                        toggle(
                            self.config.reflection,
                            |this: &mut Self, on, cx| {
                                this.config.reflection = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        "Shadows",
                        Some("A soft shadow under every cover"),
                        toggle(
                            self.config.shadow,
                            |this: &mut Self, on, cx| {
                                this.config.shadow = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        "Glow",
                        Some("Pool the accent color behind the centered cover; with the art tint on it takes the playing album's color"),
                        toggle(
                            self.config.glow,
                            |this: &mut Self, on, cx| {
                                this.config.glow = on;
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

impl QueryFilter for ArtPanel {
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

impl EventEmitter<PanelEvent> for ArtPanel {}

impl Focusable for ArtPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for ArtPanel {
    fn panel_name(&self) -> &'static str {
        "art view"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Album Carousel")
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

    /// The shelf serves cover context menus over the whole body, so the tab
    /// panel's body right-click stays out; the panel dropdown rides along
    /// after the play items, the library's arrangement.
    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

    /// The layout dump carries the panel's config; the builder registered in
    /// `workspace::register_panels` reads it back.
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
        config.center = self.goal.round().max(0.) as usize;
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
        // Follow the shared search query, or filter by this shelf's own box.
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
                ArtPanel::new(state, config, window, cx)
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

impl Render for ArtPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl ArtPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // A pending box reset (a source toggle or a shared-query change)
        // lands here, where a window exists to set the input's text.
        if self.resync_box {
            self.resync_box = false;
            self.sync_query_box(window, cx);
        }
        let max = self.max_index();
        let step = self.step_px();

        // The frame-by-frame motion: a drag scrubs `pos` live, a released
        // scrub coasts then snaps to the nearest cover, and a directed move
        // eases toward its goal. Everything steps here and requests the next
        // frame only while something is still moving.
        let dt = self.last_tick.elapsed().as_secs_f32().min(0.05);
        self.last_tick = Instant::now();
        let mut moving = false;
        if self.flick.is_dragging() {
            // `pos` is driven by the drag hook below; hold here.
            moving = true;
        } else if let Some(dx) = self.flick.coast(dt) {
            self.pos = (self.pos - dx / step).clamp(0., max);
            moving = true;
        } else {
            if self.coasting {
                // A free scrub just settled: pick the nearest cover to rest
                // on, and publish the album it landed on.
                self.goal = self.pos.round().clamp(0., max);
                self.coasting = false;
            }
            let diff = self.goal - self.pos;
            if diff.abs() > 0.001 {
                // Cover 92% of the remaining distance every tenth of a second.
                let ease = 1.0 - (0.08_f32).powf(dt * 10.0);
                self.pos += diff * ease.clamp(0., 1.);
                moving = true;
            } else {
                self.pos = self.goal;
            }
        }
        // A settle at a whole cover publishes it, once, when the album at
        // the center actually changed and a user move asked for it.
        if !moving {
            let c = self.pos.round().max(0.) as usize;
            if self.publish_pending && self.centered != Some(c) {
                self.select_only(c, cx);
            }
            self.publish_pending = false;
        }

        // The dim fade: each cover's opacity eases toward its target, the
        // same exponential approach. Only the covers with a fade in flight
        // and the visible window (whose targets shift as the shelf moves)
        // step here; the rest of the shelf sits frozen off-screen, where its
        // opacity does not show, until it scrolls back on. A big library's
        // off-shelf covers cost nothing. Frames only while one is moving.
        let dim_step = 1.0 - (0.08_f32).powf(dt * 10.0);
        let last = self.cells.len().saturating_sub(1) as i64;
        let lo = (self.pos.floor() as i64 - VIS).clamp(0, last);
        let hi = (self.pos.ceil() as i64 + VIS).clamp(0, last);
        for ix in lo..=hi {
            self.dimming.insert(ix as usize);
        }
        let mut dimming = std::mem::take(&mut self.dimming);
        dimming.retain(|&ix| {
            let target = self.dim_target(ix);
            let Some(cell) = self.cells.get_mut(ix) else {
                return false;
            };
            let Some(current) = cell.dim else {
                return false;
            };
            let d = target - current;
            if d.abs() < 0.005 {
                cell.dim = Some(target);
                false
            } else {
                cell.dim = Some(current + d * dim_step);
                moving = true;
                true
            }
        });
        self.dimming = dimming;
        if moving {
            window.request_animation_frame();
        }

        // The search lives in the tab bar via title_suffix while the panel
        // shares a group; solo or popped out there is no header at all, so it
        // renders as a toolbar in the body instead.
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
                .child(
                    if self.effective_query(cx).is_empty() && self.effective_filter(cx).is_empty() {
                        "The library is empty"
                    } else {
                        "No matches"
                    },
                )
                .into_any_element()
        } else {
            let (w, h) = self.frame();
            let hero = self.hero_side();
            let axis = self.axis();
            // The scroll axis carries the stacking offset; the other holds
            // the covers centered. `cover` maps this pair onto x and y. A
            // row's floor shifts the covers up by half its strip so cover
            // and mirror center as one block; a column mirrors both sides
            // and stays centered on its own.
            let refl_shift = if self.config.reflection && axis == Axis::Horizontal {
                (hero * REFL + REFL_GAP) / 2.0
            } else {
                0.0
            };
            let (cx_px, cy_px) = (w / 2.0, (h - LABEL_H) / 2.0 - refl_shift);
            let center = self.pos.round().max(0.) as usize;

            // The visible window around the center, painted far covers first
            // so the nearer ones stack on top and take the clicks.
            let lo = (self.pos.floor() as i64 - VIS).max(0);
            let hi = (self.pos.ceil() as i64 + VIS).min(self.cells.len() as i64 - 1);
            let mut order: Vec<i64> = (lo..=hi).collect();
            order.sort_by(|a, b| {
                let da = (*b as f32 - self.pos).abs();
                let db = (*a as f32 - self.pos).abs();
                da.partial_cmp(&db).unwrap()
            });

            let mut shelf = div().relative().flex_1().min_h_0().overflow_hidden();
            if self.config.glow {
                shelf = shelf.child(self.glow(hero, cx_px, cy_px));
            }
            // The floor is its own layer: every mirror paints far to near
            // first, then every cover over all of them. Interleaving
            // mirror and cover per index let a near mirror and a far
            // cover fight over the same pixels, which read as a mirror
            // clipped mid-face; as a strict underlayer the mirrors only
            // ever sit beneath the shelf, the way a floor should.
            if self.config.reflection {
                for &ix in &order {
                    let d = ix as f32 - self.pos;
                    if let Some(mirror) = self.reflection(ix as usize, d, hero, cx_px, cy_px, cx) {
                        shelf = shelf.child(mirror);
                    }
                }
            }
            for ix in order {
                let d = ix as f32 - self.pos;
                shelf = shelf.child(self.cover(ix as usize, d, hero, cx_px, cy_px, cx));
            }
            shelf
                .child(self.label(center, cx))
                // Any press might be a scrub; the covers' clicks moved to
                // release so both can tell.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.flick.begin(event.position.along(axis));
                        this.coasting = true;
                        this.publish_pending = true;
                        this.touch_resume(cx);
                        cx.notify();
                    }),
                )
                // The wheel steps the carousel a cover at a time, banking the
                // sub-step travel so a trackpad's small deltas still land. The
                // scroll axis leads; a plain vertical wheel still drives a row
                // when that is all the mouse sends.
                .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _, cx| {
                    this.touch_resume(cx);
                    // A wheel notch arrives as 3 lines, so a line counts a
                    // third of a step and one notch moves one cover.
                    let delta = event.delta.pixel_delta(px(WHEEL_STEP / 3.0));
                    let along = f32::from(delta.along(axis));
                    let cross = f32::from(delta.along(axis.invert()));
                    let d = if along.abs() >= cross.abs() {
                        along
                    } else {
                        cross
                    };
                    this.wheel += d;
                    let mut goal = this.goal;
                    while this.wheel >= WHEEL_STEP {
                        goal += 1.;
                        this.wheel -= WHEEL_STEP;
                    }
                    while this.wheel <= -WHEEL_STEP {
                        goal -= 1.;
                        this.wheel += WHEEL_STEP;
                    }
                    let goal = goal.clamp(0., this.max_index());
                    if goal != this.goal {
                        this.goal = goal;
                        this.coasting = false;
                        this.publish_pending = true;
                        cx.notify();
                    }
                }))
                // The live scrub follows the pointer through window handlers
                // armed in a paint pass, the scrub strips' idiom. The canvas
                // exists for that paint hook and to measure the frame.
                .child(
                    canvas(
                        {
                            let weak = cx.entity().downgrade();
                            move |bounds: Bounds<Pixels>, _, cx| {
                                if let Some(this) = weak.upgrade() {
                                    this.update(cx, |this, cx| {
                                        if this.size != bounds.size {
                                            this.size = bounds.size;
                                            cx.notify();
                                        }
                                    });
                                }
                            }
                        },
                        {
                            let flick = self.flick.clone();
                            let weak = cx.entity().downgrade();
                            move |_, _, window, _| {
                                let weak = weak.clone();
                                panel::flick_on_paint_axis(&flick, axis, window, move |dx, cx| {
                                    if let Some(this) = weak.upgrade() {
                                        this.update(cx, |this, cx| {
                                            let max = this.max_index();
                                            let step = this.step_px();
                                            this.pos = (this.pos - dx / step).clamp(0., max);
                                            cx.notify();
                                        });
                                    }
                                });
                            }
                        },
                    )
                    .absolute()
                    .size_full(),
                )
                // The shelf's right-click menu, keyed off the hovered cover
                // since the builder gets no position: the hovered cover is
                // selected first so the menu acts on what is highlighted. Off
                // any cover the panel menu stands alone.
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
                        this.update(cx, |this, cx| {
                            if !this.selected.contains(&ix) {
                                this.select_only(ix, cx);
                            }
                        });
                        let ids: Vec<i64> = this.update(cx, |this, cx| this.ids_for(ix, cx));
                        let panel = weak.clone();
                        let menu = menu.item(
                            PopupMenuItem::new("Play")
                                .icon(Icon::default().path(icons::PLAY))
                                .on_click(move |_, _, cx| {
                                    if let Some(this) = panel.upgrade() {
                                        this.update(cx, |this, cx| this.play(ix, cx));
                                    }
                                }),
                        );
                        // The primary editing flow: the album into the tag
                        // editor window.
                        let state = this.read(cx).state.clone();
                        let reveal = ids.first().copied();
                        let menu = menu.item(
                            PopupMenuItem::new("Edit Tags...")
                                .icon(Icon::default().path(icons::PENCIL))
                                .on_click(move |_, _, cx| {
                                    rox_panel_api::openers::tags_editor(
                                        state.clone(),
                                        ids.clone(),
                                        cx,
                                    );
                                }),
                        );
                        // Reveal follows the album's first track, landing in
                        // that album's folder.
                        let menu = panel::reveal_item(menu, this.read(cx).state.clone(), reveal);
                        // Faceted browse: pin the search to the cover's artist,
                        // the shelf's stand-in for the artist's own shelf.
                        let menu = match this
                            .read(cx)
                            .cell_artist(ix, cx)
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
