//! The art view panel: the catalog as a cover carousel, NekoRoX's shelf.
//! One album centered and square, its neighbors shrinking, turning, and
//! tucking behind it toward both edges, so browsing reads as flipping
//! through a rack of covers. A row that scrolls left and right by default,
//! or a column that scrolls up and down by a setting. The turn is a real
//! projection through the sprite pipeline, a keystone at the angle the
//! tilt setting names, since gpui has no 3D of its own to ask for one.
//! Turned off, the shelf stays flat and square and lets the distance
//! shrink and the depth light carry the rack instead. It shares the album
//! grid's whole model,
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
    ImageSource, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ObjectFit, Pixels, RenderImage, ScrollWheelEvent, SharedString, Size, Subscription, WeakEntity,
    Window,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Side};
use image::Frame;
use rox_core::QUEUE_CAP;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::TrackKey;
use rox_library::projection::{Projection, QueryField, QUERY_FIELDS};
use rox_panel_api::actions::{TypeAheadNext, TypeAheadPrev};
use rox_panel_kit::config::{default_true, is_zero};
use rox_panel_kit::wall::{default_dim, TILE_DIM_MAX};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::discs::{self, DiscCache, DiscShape, DiscStyle};
use crate::grid::LetterSide;
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

/// Covers drawn to each side of the centered one, and the most the setting
/// will draw. Every cover past the count is off the shelf, so the deeper
/// the rack the more work each frame costs for less and less that reads.
const VIS: u8 = 5;
const VIS_MAX: f32 = 16.;

/// How much each step out from center shrinks a cover, multiplied per unit
/// of distance, floored at [`MIN_SCALE`].
const SHRINK: f32 = 0.86;
const MIN_SCALE: f32 = 0.5;

/// The first flank's center, in percent of the hero's edge, and the range
/// the setting scrubs it across. Under 100 the neighbor tucks behind the
/// hero; past it the flanks clear the hero and leave it standing in its own
/// space, the way NekoRoX's shelf sat.
const SHIFT0: f32 = 56.;
const SHIFT_MIN: f32 = 20.;
const SHIFT_MAX: f32 = 140.;
/// Each further cover's step past the first, same units, and the range its
/// own setting scrubs across. This one also sets the drag mapping: it's how
/// far the shelf travels per cover.
const STEP: f32 = 30.;
const STEP_MIN: f32 = 5.;
const STEP_MAX: f32 = 100.;

/// How dark the shading over a fully turned cover goes, as a fraction of
/// the panel's background: the light the shelf reads as coming from the
/// front.
const TURN_SCRIM: f32 = 90. / 255.;

/// How much of the way into the background the deepest cover in the rack
/// sits, in percent of full brightness: 100 leaves the whole shelf lit
/// evenly, 0 sinks the back of it into the panel. The covers between the
/// center and the back share the distance evenly, so at the shipped 20
/// over five covers each step costs 16 points, which is the ramp the
/// carousel has always had.
const RECEDE: f32 = 20.;

/// How much of a cover's own step the last one in the rack spends fading
/// out: the ramp that carries it to nothing right as it leaves the window,
/// so covers arrive and leave instead of popping.
const EDGE_FADE: f32 = 1.0;
/// Below this a cover is too faint to mean anything, so it stops taking
/// clicks and hovers along with it.
const HIT_OP: f32 = 0.08;

/// Covers past the shelf's own depth that get their artwork and disc bake
/// started early, so they arrive dressed instead of catching up on screen.
const WARM: i64 = 4;

/// The label strip's height under the hero, reserved out of the panel so
/// the covers sit above it.
const LABEL_H: f32 = 40.;

/// The reflection floor: how far a cover's mirror extends past its lower
/// edge, as a fraction of the cover, and how bright the mirror starts
/// before it fades to nothing.
const REFL: f32 = 0.32;
const REFL_OP: f32 = 0.45;
/// The seam between a cover and its mirror, in px.
const REFL_GAP: f32 = 2.;

/// The perspective turn: how far a flank cover rotates about its cross
/// axis, in degrees, the range the setting scrubs it across, and the
/// projection's focal length in hero edges. The tilt reaches its full angle
/// one step out and holds it from there. Past 85 a cover is edge-on
/// and gone, so the strip stops short of it either way: negative swings the
/// far edge toward you instead of the inner one, and the rack turns outward.
const TILT: f32 = 55.;
const TILT_MAX: f32 = 85.;
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
    /// Size the hero off the cross axis alone, letting the flank covers
    /// run off the panel edge. The default fit shrinks the hero to keep
    /// the flanks inside, which reads as wasted space in a narrow panel.
    #[serde(default)]
    pub fill: bool,
    /// Bring the playing album to the center when the track changes.
    #[serde(default)]
    pub follow_playing: bool,
    /// After the shelf goes untouched for a spell, glide the playing album
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
    /// sprite pipeline. On by default; off leaves the rack flat and
    /// square, carried by the distance shrink and the depth light alone,
    /// which is also the only mode where art rounding applies.
    #[serde(default = "default_true")]
    pub perspective: bool,
    /// How far a flank cover turns away from you, in degrees. Only the
    /// perspective projection reads it; nothing turns with it off.
    #[serde(default = "default_tilt")]
    pub tilt: f32,
    /// How far the first flank sits from the hero, in percent of the hero's
    /// edge. Low values tuck the neighbors behind the hero, high ones push
    /// them off it and pad the center cover on both sides.
    #[serde(default = "default_spacing")]
    pub spacing: f32,
    /// The gap between the covers behind the first flank, in percent of the
    /// hero's edge: how tightly the rack stacks once it's past the center.
    /// It's also the drag mapping, so a wider stack scrolls further per
    /// cover.
    #[serde(default = "default_stride")]
    pub stride: f32,
    /// Covers drawn to each side of the center. The last one fades out as
    /// it leaves, so a low count reads as a short shelf rather than a
    /// clipped one.
    #[serde(default = "default_visible")]
    pub visible: u8,
    /// How lit the deepest cover in the rack is, in percent. It's painted
    /// as a wash toward the panel's own background rather than as
    /// transparency: the covers behind the hero overlap each other, and
    /// see-through ones would show the rack's whole stack through the
    /// nearest face.
    #[serde(default = "default_recede")]
    pub recede: f32,
    /// A letter rail along the shelf's edge: the album artists' initials,
    /// each a click that jumps the carousel to its first album.
    #[serde(default)]
    pub letters: bool,
    /// Keep the rail to one line that scrolls instead of wrapping, for
    /// libraries whose scripts spill past one row of initials.
    #[serde(default)]
    pub letters_compact: bool,
    /// Which edge of the shelf the rail hangs on. The far edge by default.
    #[serde(default)]
    pub letters_side: LetterSide,
    /// Where the hero's caption sits: over the shelf's top, right under
    /// the cover, along the panel's bottom, or nowhere.
    #[serde(default)]
    pub label: LabelPos,
    /// The album at the center when the layout was saved, so a relaunch
    /// reopens the shelf where it was left. A cell index.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub center: usize,
}

/// The shelf's shipped turn and spacing, for the configs that predate the
/// knobs and for a reset.
fn default_tilt() -> f32 {
    TILT
}

fn default_spacing() -> f32 {
    SHIFT0
}

fn default_stride() -> f32 {
    STEP
}

fn default_visible() -> u8 {
    VIS
}

fn default_recede() -> f32 {
    RECEDE
}

/// Where the hero's caption goes. Center, the default, hangs it right
/// under the cover so it reads as the album's own caption.
#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LabelPos {
    Top,
    #[default]
    Center,
    Bottom,
    Hidden,
}

impl Default for ArtConfig {
    fn default() -> Self {
        ArtConfig {
            chrome: PanelChrome::default(),
            query: String::new(),
            search: false,
            query_source: QuerySource::default(),
            vertical: false,
            fill: false,
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
            tilt: default_tilt(),
            spacing: default_spacing(),
            stride: default_stride(),
            visible: default_visible(),
            recede: default_recede(),
            letters: false,
            letters_compact: false,
            letters_side: LetterSide::default(),
            label: LabelPos::default(),
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
    /// target every frame. None until the cover's first paint, which starts
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

/// Whether a point sits inside a convex quad, corners in order. Every
/// edge crossed with the point has to lean the same way; a keystone and a
/// plain rect are both convex, so the one test covers the shelf's shapes.
/// A point on an edge counts as in, which is what a click on the seam
/// between two covers wants.
fn inside(quad: &[[f32; 2]; 4], p: [f32; 2]) -> bool {
    let (mut left, mut right) = (false, false);
    for i in 0..4 {
        let a = quad[i];
        let b = quad[(i + 1) % 4];
        let side = (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        left |= side > 0.;
        right |= side < 0.;
    }
    !(left && right)
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
    /// The window-edge fade, before the dim mode multiplies in. The only
    /// transparency in the shelf's own depth cue.
    fade: f32,
    /// How far the cover has sunk into the background, painted as a wash.
    recede: f32,
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
    /// The letter rail's entries: each distinct initial in the view and
    /// the first cell under it, rebuilt with the cells. The canonical
    /// order sorts by folded artist name, so the initials arrive grouped.
    letters: Vec<(SharedString, usize)>,
    /// The baked disc faces while a disc style is on, keyed by art path
    /// and filled off-thread as covers come into view.
    discs: DiscCache,
    /// The query editor, the shared search box; `config.query` tracks its
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
    /// album it settles on, not every one it passes.
    publish_pending: bool,
    /// Wheel travel banked toward the next [`WHEEL_STEP`].
    wheel: f32,
    /// The panel's measured content box, the carousel's frame. The dock
    /// hosts panels cached, so a resize repaints without re-rendering; a
    /// measuring canvas compares against this and notifies on drift.
    size: Size<Pixels>,
    /// The shelf's top-left in window coordinates, measured by the same
    /// canvas: what turns a pointer position into the shelf-space the
    /// covers are laid out in.
    origin: gpui::Point<Pixels>,
    /// Every painted cover's outline in shelf space, nearest first. A
    /// turned cover's box holds a good deal of floor it doesn't cover, so
    /// the click has to land on the shape rather than on the box gpui
    /// hit-tests; this is what it lands on.
    hits: Vec<(usize, [[f32; 2]; 4])>,
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
    /// The turn angle, hero spacing, stack stride, and cover count strips,
    /// same window.
    tilt_scrub: ScrubState,
    spacing_scrub: ScrubState,
    stride_scrub: ScrubState,
    visible_scrub: ScrubState,
    recede_scrub: ScrubState,
    /// The one readout being typed into across the settings sliders.
    value_edit: panel::ValueEdit,
    /// A failed play, shown in a strip until the next play succeeds.
    error: Option<SharedString>,
    /// A pending box reset from a source toggle or a shared-query change;
    /// applied on the next render, where a window exists to set the input.
    resync_box: bool,
    /// The tracks this panel is pinned to while following the selection.
    /// Runtime only: a restore re-pins from whatever is picked then.
    selection_ids: Vec<i64>,
    /// The type-ahead phrase and when its last keystroke landed, so typing
    /// while the shelf has focus jumps to the album by prefix, and a quick
    /// run of keys grows one phrase instead of restarting each stroke.
    type_ahead: String,
    type_ahead_at: Option<Instant>,
    focus: FocusHandle,
    /// The tab panel this panel is currently in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _library_changed: Subscription,
    _thumbs_changed: Subscription,
    _search_events: Subscription,
    _query_changed: Subscription,
    _selection_changed: Subscription,
    _player_changed: Subscription,
    /// Drops the phrase when focus leaves the panel, so tab goes back to
    /// walking panels instead of cycling a phrase from a past visit.
    _type_ahead_blur: Subscription,
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
        // Arriving thumbnails notify the service; repaint so covers fill in.
        let _thumbs_changed = cx.observe(&state.thumbs, |_, _, cx| cx.notify());
        // A shelf restored as global opens showing the shared query; an
        // own-query one shows its own.
        let initial = match config.query_source {
            QuerySource::Global => state.query.read(cx).text().to_string(),
            QuerySource::Local | QuerySource::Selection => config.query.clone(),
        };
        let search =
            cx.new(|cx| SearchBox::new(rox_i18n::t!("query-search"), &initial, window, cx).small());
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
        let focus = cx.focus_handle().tab_stop(true);
        // The phrase outlives its badge, so it needs an end: leaving the
        // panel drops it, which is also what hands tab back to traversal.
        let panel = cx.weak_entity();
        let _type_ahead_blur = window.on_focus_out(&focus, cx, move |_, _, cx| {
            panel
                .update(cx, |this: &mut ArtPanel, cx| {
                    this.clear_type_ahead(cx);
                })
                .ok();
        });
        let mut this = ArtPanel {
            state,
            config,
            view: Arc::new(Vec::new()),
            cells: Vec::new(),
            dimming: HashSet::new(),
            letters: Vec::new(),
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
            origin: gpui::Point::default(),
            hits: Vec::new(),
            flick: FlickState::default(),
            last_tick: Instant::now(),
            resume_idle: ResumeIdle::default(),
            playing_key: None,
            playing_ix: None,
            playing: false,
            // Suppress the launch settle's publish: a restore reopens a
            // position, it doesn't reach out and reselect.
            centered: Some(start as usize),
            rounding_scrub: ScrubState::default(),
            dim_scrub: ScrubState::default(),
            tilt_scrub: ScrubState::default(),
            spacing_scrub: ScrubState::default(),
            stride_scrub: ScrubState::default(),
            visible_scrub: ScrubState::default(),
            recede_scrub: ScrubState::default(),
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
            _search_events,
            _query_changed,
            _selection_changed,
            _player_changed,
            _type_ahead_blur,
        };
        this.rebuild(cx);
        // A duplicate opens with a track already playing; pick it up now
        // instead of waiting for the next track change.
        this.sync_playing(cx);
        this
    }

    /// Follow the player: on a track change, center the album it belongs
    /// to, and keep the dim mode's facts fresh. The compares keep the per-tick
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

    /// What the idle wake does: glide the playing album back to the center,
    /// so long as the resume is still on. The clock only fires this once the
    /// shelf has gone untouched a full window, a gesture in between having
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
    /// settle to publish, so the album it settles on goes to the selection.
    fn navigate(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.goal = ix as f32;
        self.coasting = false;
        self.publish_pending = true;
        cx.notify();
    }

    /// Step the shelf `delta` covers, clamped to the ends. Measured off
    /// `goal` rather than `pos` so a held arrow banks its steps instead of
    /// fighting the ease back to where the last one started.
    fn step_cover(&mut self, delta: i64, cx: &mut Context<Self>) {
        let last = self.max_index() as i64;
        let target = (self.goal.round() as i64 + delta).clamp(0, last) as usize;
        self.center_on(target, cx);
    }

    /// Center a cover and select it, the single click's pair of moves.
    fn center_on(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.cells.len() {
            return;
        }
        self.select_only(ix, cx);
        self.navigate(ix, cx);
    }

    /// Browse from the keyboard while the shelf is focused: the arrows step
    /// a cover, home and end run to the ends, and enter plays whatever is
    /// centered. Both arrow pairs step, the wheel's rule above: the shelf
    /// has one dimension, so there's nothing else for the cross pair to
    /// mean. Modifiers pass through so the workspace keeps its shortcuts.
    fn on_panel_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.alt {
            return;
        }
        // Browsing by keyboard is browsing, so it restarts the idle clock
        // the same as a wheel or a drag.
        self.touch_resume(cx);
        match keystroke.key.as_str() {
            "left" | "up" => self.step_cover(-1, cx),
            "right" | "down" => self.step_cover(1, cx),
            // A page is the covers actually on screen to one side, so it
            // lands the shelf just past what you were looking at.
            "pageup" => self.step_cover(-self.visible(), cx),
            "pagedown" => self.step_cover(self.visible(), cx),
            "home" => self.center_on(0, cx),
            "end" => self.center_on(self.cells.len().saturating_sub(1), cx),
            "enter" => {
                let ix = self.goal.round().max(0.) as usize;
                if ix < self.cells.len() {
                    self.play(ix, cx);
                }
            }
            "escape" => {
                self.clear_type_ahead(cx);
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
                // binding, which the shelf otherwise inherits unscoped.
                cx.stop_propagation();
                self.type_to(text.clone(), cx);
            }
        }
    }

    /// Split a leading `field:` pin off the phrase, the query syntax's
    /// vocabulary: `album:` or `title:` for the album name, `artist:` or
    /// `albumartist:` for the artist. Fields with no text on a cover fall
    /// through and the phrase reads literally.
    fn type_ahead_pin(phrase: &str) -> Option<(bool, &str)> {
        let (name, rest) = phrase.split_once(':')?;
        let (_, field) = QUERY_FIELDS
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))?;
        match field {
            QueryField::Album | QueryField::Title => Some((false, rest)),
            QueryField::Artist | QueryField::AlbumArtist => Some((true, rest)),
            _ => None,
        }
    }

    /// Grow or restart the type-ahead phrase and center the cover it names.
    /// A fresh phrase starts past the current cover, so the same letter
    /// steps to the next match; a grown one re-tests the current cover so
    /// refining a match stays put. The phrase matches the start of any word
    /// in the cover's album name or artist; a `field:` pin narrows it to
    /// one.
    fn type_to(&mut self, text: String, cx: &mut Context<Self>) {
        let grown = panel::type_ahead_grow(&mut self.type_ahead, &mut self.type_ahead_at, text);
        // The badge shows the phrase now and leaves when the window
        // lapses; a miss below still updated it, so repaint either way.
        panel::type_ahead_fade(cx);
        cx.notify();
        let len = self.cells.len();
        if len == 0 {
            return;
        }
        let needle = self.type_ahead.to_lowercase();
        let pin = Self::type_ahead_pin(&needle);
        // A grown phrase re-tests the centered cover; a fresh one starts
        // past it, so the same first letter steps to the next match.
        let start = match grown.then_some(self.goal.round().max(0.) as usize) {
            Some(ix) => ix,
            None => (self.goal.round().max(0.) as usize + 1).min(len.saturating_sub(1)),
        };
        let hit = {
            let library = self.state.library.read(cx);
            library.projection().and_then(|projection| {
                (0..len)
                    .map(|off| (start + off) % len)
                    .find(|&ix| self.type_hit(projection, ix, pin, &needle))
            })
        };
        if let Some(ix) = hit {
            self.center_on(ix, cx);
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
    /// stamp alone: the badge belongs to typing, so a run of tabs steps
    /// silently rather than reviving it.
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
        let pin = Self::type_ahead_pin(&needle);
        let anchor = Some(self.goal.round().max(0.) as usize);
        let hit = {
            let library = self.state.library.read(cx);
            library.projection().and_then(|projection| {
                panel::type_ahead_scan(len, anchor, back)
                    .find(|&ix| self.type_hit(projection, ix, pin, &needle))
            })
        };
        if let Some(ix) = hit {
            self.center_on(ix, cx);
        }
    }

    /// Whether one cover's album matches the phrase, [`Self::type_to`]'s
    /// rules: the pinned text alone when pinned, a word start in either
    /// otherwise.
    fn type_hit(
        &self,
        projection: &Projection,
        ix: usize,
        pin: Option<(bool, &str)>,
        needle: &str,
    ) -> bool {
        let Some(&row) = self
            .cells
            .get(ix)
            .and_then(|cell| self.view.get(cell.start))
        else {
            return false;
        };
        let resolved = projection.resolve(row);
        let album = resolved.album;
        let artist = resolved.album_artist;
        match pin {
            Some((true, rest)) => panel::type_ahead_hit(artist, rest),
            Some((false, rest)) => panel::type_ahead_hit(album, rest),
            None => panel::type_ahead_hit(album, needle) || panel::type_ahead_hit(artist, needle),
        }
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
    /// being iterated directly. Otherwise an album's scattered rows would
    /// split into duplicate covers. Breaks on the album artist, not the
    /// track artist, so a compilation stays one cover.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.cells.clear();
        self.dimming.clear();
        self.selected.clear();
        // The settle dedupe keys on a cell index, and the rebuild may have
        // just reordered or refiltered the cells under it: the same index
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
        // The rail's letters, one entry per distinct initial. Cheap enough
        // to keep fresh whether or not the rail shows, so the toggle is
        // instant.
        self.letters.clear();
        if let Some(projection) = self.state.library.read(cx).projection() {
            for (ix, cell) in self.cells.iter().enumerate() {
                let row = self.view[cell.start] as usize;
                // The same key the ordering runs on, so a sort-tagged name
                // lands under the letter the rail names.
                let name = projection
                    .album_artists
                    .sort_key(projection.album_artist[row] as usize);
                let letter = panel::letter_initial(name);
                if self.letters.last().map(|(l, _)| l.as_ref()) != Some(letter.as_str()) {
                    self.letters.push((SharedString::from(letter), ix));
                }
            }
        }
        // A shorter view (a query) can leave the center past the end. Only
        // re-clamp once the projection is loaded, though: on a cold start it
        // hasn't arrived, so there are no cells yet and clamping here would
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
                // track's art comes back first.
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
        // The cap along the scroll axis: the fit keeps the first flanks
        // inside the panel, fill spends the whole axis on the hero and
        // lets the flanks clip at the edge.
        // Wider spacing throws the flanks further out, so the fit trades
        // hero size back for them in the same proportion; it never spends
        // more of the axis than fill would.
        let cap = if self.config.fill {
            1.0
        } else {
            (0.42 * SHIFT0 / self.config.spacing.max(1.)).min(1.0)
        };
        match self.axis() {
            // A row: covers as tall as the band, capped by the width.
            Axis::Horizontal => (avail_h * 0.9 / floor).min(w * cap),
            // A column: covers as wide as the panel, capped by its length.
            Axis::Vertical => (w * 0.86 / sides).min(avail_h * cap),
        }
        .max(48.)
    }

    /// Px of drag along the scroll axis per cover of travel, the coast and
    /// pointer mapping.
    fn step_px(&self) -> f32 {
        (self.hero_side() * self.config.stride / 100.).max(1.)
    }

    /// Covers drawn to each side of the center. At least one, or the shelf
    /// is a single cover with nothing to browse.
    fn visible(&self) -> i64 {
        self.config.visible.max(1) as i64
    }

    /// Whether cover `ix` is in the receded set: the covers the focus
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
    /// until it finishes. The bytes are the thumb's own, so the face is as
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
    /// the div actually ends up.
    fn quad_canvas(
        quad: [[f32; 2]; 4],
        image: Option<Arc<gpui::Image>>,
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
                    // No bake to paint: a plain cover always has an image
                    // to fall back to; the disc styles' own placeholder
                    // path never reaches here without one either, since
                    // `blank_disc` stands in as a bake, not as this.
                    None => image.as_ref().and_then(|image| {
                        ImageSource::from(image.clone())
                            .use_data(None, window, cx)
                            .and_then(|result| result.ok())
                            .map(|data| {
                                let source = square_source(&data);
                                (data, source)
                            })
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

    /// Start the disc bakes for the covers just off the shelf, so they're
    /// finished before those covers scroll on. A bake claimed on a cover's
    /// first paint lands a beat after it, which is why a scrolling shelf
    /// shows flat art that pops into discs behind the pointer; the bake is
    /// only a few milliseconds, so a few covers of lead is enough to cover
    /// any speed a hand scrolls at. Everything here is a cache hit once
    /// warmed: a claimed path is skipped, a finished one is served.
    fn warm_discs(&mut self, cx: &mut Context<Self>) {
        if self.config.disc_style == DiscStyle::Off {
            return;
        }
        let last = self.cells.len().saturating_sub(1) as i64;
        let reach = self.visible() + WARM;
        let center = self.pos.round() as i64;
        let lo = (center - reach).clamp(0, last);
        let hi = (center + reach).clamp(0, last);
        // Outward from the center, so when the pool is full the slots have
        // already gone to the covers nearest the eye. Walking the window in
        // index order would hand them to whichever end of the shelf happens
        // to sort first, which is the wrong end half the time.
        let mut window: Vec<i64> = (lo..=hi).collect();
        window.sort_by_key(|ix| (ix - center).abs());
        for ix in window {
            let ix = ix as usize;
            let Some(path) = self.art_path(ix, cx) else {
                continue;
            };
            // The thumb is the bake's input, so this warms the loads a step
            // ahead too; a miss just reports Pending and the next frame
            // picks the bake up once it lands.
            let Thumb::Ready(image) = self
                .state
                .thumbs
                .update(cx, |thumbs, cx| thumbs.get(&path, cx))
            else {
                continue;
            };
            self.disc_of(path, image, cx);
        }
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

    /// The only real transparency a cover gets: the ramp that empties the
    /// last one as it crosses the window's edge. The window runs a cover
    /// deeper than the count, which is the room the ramp needs; without it
    /// the outermost cover would blink in and out at full strength.
    fn edge_fade(&self, a: f32) -> f32 {
        let depth = self.visible() as f32;
        ((depth + EDGE_FADE - a) / EDGE_FADE).clamp(0., 1.)
    }

    /// How far into the background a cover at distance `a` has sunk, 0 at
    /// the center to 1 gone. The center is always full, the deepest cover
    /// lands on the setting, and the rack divides the distance evenly, so
    /// the shelf reads the same depth however many covers deep it runs.
    /// This is a wash the covers paint over their own faces, never
    /// transparency: a receded cover still hides the one behind it.
    fn recede(&self, a: f32) -> f32 {
        let depth = self.visible() as f32;
        let back = (self.config.recede / 100.).clamp(0., 1.);
        1.0 - (1.0 - (1.0 - back) * (a / depth)).clamp(back, 1.0)
    }

    /// A cover's outline in shelf space: the projected keystone with
    /// perspective on, the flat rect with it off. Corners run the same way
    /// round either way, so the containment test doesn't care which it got.
    fn outline(&self, d: f32, hero: f32, cx_px: f32, cy_px: f32) -> [[f32; 2]; 4] {
        if self.config.perspective {
            return self.quad(d, hero, cx_px, cy_px);
        }
        let p = self.placement(d, hero, cx_px, cy_px);
        [
            [p.left, p.top],
            [p.left + p.w, p.top],
            [p.left + p.w, p.top + p.h],
            [p.left, p.top + p.h],
        ]
    }

    /// The cover a pointer is over, in window coordinates: the nearest one
    /// whose outline holds the point. The list is built nearest-first as
    /// the shelf paints, so the first hit is the one on top, the same
    /// cover the eye picks.
    fn hit(&self, at: gpui::Point<Pixels>) -> Option<usize> {
        let p = [
            f32::from(at.x - self.origin.x),
            f32::from(at.y - self.origin.y),
        ];
        self.hits
            .iter()
            .find(|(_, quad)| inside(quad, p))
            .map(|(ix, _)| *ix)
    }

    /// A cover's box at distance `d` from the center: position, size, the
    /// distance fade before the dim mode multiplies in, and how far the
    /// cover has turned away. The cover and its reflection read the same
    /// numbers, which keeps the mirror under its cover through every
    /// scrub frame.
    fn placement(&self, d: f32, hero: f32, cx_px: f32, cy_px: f32) -> Placement {
        let a = d.abs();
        let scale = SHRINK.powf(a).max(MIN_SCALE);
        // Only the projection turns a cover. The flat shelf used to fake
        // one by squashing the face along the scroll axis, which stretched
        // the art into a letterbox and read as a squeezed cover rather
        // than a turned one; it keeps its covers square and lets the
        // scale and the depth light carry the distance instead. The turn
        // still rides along for the projection's own shading.
        let turn = if self.config.perspective {
            a.clamp(0., 1.)
        } else {
            0.
        };
        // Square either way: the box is the hero's edge taken down by the
        // distance shrink.
        let side = hero * scale;
        let off = self.offset_units(d) * hero;
        let (cover_x, cover_y, w, h) = match self.axis() {
            Axis::Horizontal => (cx_px + off, cy_px, side, side),
            Axis::Vertical => (cx_px, cy_px + off, side, side),
        };
        Placement {
            left: cover_x - w / 2.0,
            top: cover_y - h / 2.0,
            w,
            h,
            fade: self.edge_fade(a),
            recede: self.recede(a),
            turn,
        }
    }

    /// A cover's projected quad at distance `d`: corners clockwise from
    /// the texture's top-left, in shelf coordinates. The cover rotates
    /// about its cross axis through its center, inner edge toward the
    /// viewer, and projects with a focal length a few covers deep: the
    /// keystone a flat shelf can't draw. The shrink keeps the near edge
    /// under the hero's height, so the flanks never outgrow the band.
    fn quad(&self, d: f32, hero: f32, cx_px: f32, cy_px: f32) -> [[f32; 2]; 4] {
        let a = d.abs();
        let scale = SHRINK.powf(a).max(MIN_SCALE);
        let half = hero * scale / 2.0;
        let theta = self.config.tilt.to_radians() * a.clamp(0., 1.) * d.signum();
        let (sin, cos) = theta.sin_cos();
        let focal = hero * FOCAL;
        let off = self.offset_units(d) * hero;
        // An edge at offset `u` along the scroll axis is at depth
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
    /// the first neighbor sits where the spacing knob puts it, each further
    /// one steps out past that by a fixed stride, so widening the hero's
    /// gap doesn't pull the whole stack apart with it.
    fn offset_units(&self, d: f32) -> f32 {
        let s = d.signum();
        let a = d.abs();
        let shift = self.config.spacing / 100.;
        if a <= 1.0 {
            s * shift * a
        } else {
            s * (shift + self.config.stride / 100. * (a - 1.0))
        }
    }

    /// The quiet stand-in for art that isn't ready to show yet, whatever
    /// the reason: no thumb, no track, or (with a disc style on) a thumb
    /// that hasn't finished its bake.
    fn placeholder() -> AnyElement {
        div()
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
            .into_any_element()
    }

    /// One cover in the carousel: placed absolutely by its distance `d` from
    /// the center, scaled and turned and dimmed by it. Pending and missing
    /// art use the same quiet placeholder, so an arriving cover fills in
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
        // The first paint starts at the dim target directly; from then on the
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
            recede,
            turn,
        } = self.placement(d, hero, cx_px, cy_px);
        let opacity = fade * dim;
        // The turn's shading and the depth wash are one coat: two alphas
        // over the same background compose to this, so the darker of them
        // leads and neither doubles the other.
        let scrim = 1.0 - (1.0 - turn * TURN_SCRIM) * (1.0 - recede);
        let wash = palette::alpha(palette::bg_root(), (scrim * 255.) as u8);
        let disc_on = self.config.disc_style != DiscStyle::Off;
        let persp = self.config.perspective;
        // With perspective on the div spans the keystone's box and the
        // canvas inside paints the real shape; off, the flat square box.
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
        // The disc face once its own bake finishes; short of that, the
        // style's own blank plate, so a disc-dressed cover never shows a
        // flat square pretending to be a disc, nor waits on anything
        // per-album before it reads as a disc at all. The blank needs no
        // work of its own (built once, shared, never touches the pool in
        // `warm_discs`), so it's there the instant the style turns on,
        // whether or not this cover's own thumb has even loaded yet.
        let baked = if disc_on {
            match (&thumb, &path) {
                (Thumb::Ready(image), Some(path)) => self.disc_of(path.clone(), image.clone(), cx),
                _ => None,
            }
            .or_else(|| discs::blank_disc(self.config.disc_style))
        } else {
            None
        };
        // Every flat cover keeps its aspect and crops to its square box.
        // The flanks used to Fill instead, stretching the art into the
        // squashed box that faked their turn; without the squash there's
        // nothing to stretch into and the hero's own fit is right for all
        // of them.
        let fit = ObjectFit::Cover;
        let desaturated = self.desaturated(ix);
        let disc_shown = baked.is_some();
        // Whether there's actual art (a bake, real or blank, or a loaded
        // thumb) for this cell at all, as opposed to the bare "nothing
        // here yet" placeholder. The turn and depth wash exist to shade
        // art; laid over the placeholder's already-dim icon and backdrop
        // instead, it read as the placeholder itself losing opacity the
        // further out it sat; the placeholder holds one flat brightness
        // wherever it lands instead.
        let has_content = disc_shown || matches!(thumb, Thumb::Ready(_));
        let quad_painted = quad.is_some() && has_content;
        let content: AnyElement = match (quad, thumb, baked) {
            // A disc face (real or the blank stand-in) through the
            // keystone: the wash bakes the turn scrim and the depth into
            // the sprite, shaped to the quad the way an overlay div can't
            // be. No thumb needed here; the bake carries its own pixels.
            (Some(quad), _, Some(bake)) => {
                let rel = quad.map(|[x, y]| [x - left, y - top]);
                Self::quad_canvas(
                    rel,
                    None,
                    Some(bake),
                    desaturated,
                    [1.; 4],
                    wash.into(),
                    None,
                )
            }
            // No disc style: the keystone paints the flat thumb itself.
            (Some(quad), Thumb::Ready(image), None) => {
                let rel = quad.map(|[x, y]| [x - left, y - top]);
                Self::quad_canvas(
                    rel,
                    Some(image),
                    None,
                    desaturated,
                    [1.; 4],
                    wash.into(),
                    None,
                )
            }
            // The bake is square with the disc touching its edges and
            // holding its own alpha, so the fill stretch turns it with
            // the box and nothing needs clipping into shape.
            (None, _, Some(disc)) => img(disc)
                .size_full()
                .object_fit(ObjectFit::Fill)
                .grayscale(desaturated)
                .into_any_element(),
            (None, Thumb::Ready(image), None) => img(image)
                .size_full()
                // Cover only crops if something masks it; gpui paints
                // the overrun otherwise, and the hero would spill onto
                // the covers turned beside it.
                .overflow_hidden()
                .object_fit(fit)
                .grayscale(desaturated)
                .rounded(radius)
                .into_any_element(),
            _ => Self::placeholder(),
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
            // A finished disc face has its own alpha, and a keystone
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
            // Hover and click are the shelf's, not the cover's: a turned
            // cover's box carries floor it doesn't paint, and gpui hands
            // the box the pointer. The shelf tests the outlines instead.
            .cursor_pointer()
            .child(content)
            // The same coat the keystone bakes in, as an overlay: it
            // deepens with the turn and with the cover's distance back, so
            // an angled cover reads as lit from the front like the hero and
            // a deep one sits in the panel's shadow without going
            // see-through.
            .when(!quad_painted && has_content && scrim > 0.008, |d| {
                d.child(div().absolute().inset_0().rounded(radius).bg(wash))
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
    /// [`REFL`] of the way out: a true alpha fade, so the glow and
    /// whatever else lies underneath shows through. A row gets the one
    /// floor below; a column has no floor to stand on, so it mirrors both
    /// side edges and stays symmetric. With perspective on the mirrors
    /// keystone with their cover, each column reflected past its own
    /// edge, which is exactly the 3D mirror. Only real art reflects; a
    /// placeholder's floor stays bare so an arriving cover's mirror fills
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
        let image = match thumb {
            Thumb::Ready(image) => Some(image),
            _ => None,
        };
        let Placement {
            left,
            top,
            w,
            h,
            fade,
            recede,
            turn: _,
        } = self.placement(d, hero, cx_px, cy_px);
        // The mirror takes its cover's depth as the same wash, so the floor
        // sinks back with the rack instead of thinning out under it. The
        // turn's own shading stays off the floor: a mirror is already the
        // dim half of the reflection.
        let wash = palette::alpha(palette::bg_root(), (recede * 255.) as u8).into();
        // The dim as the cover painted it; before a first paint the target
        // stands in, read-only, so the mirror never races the fade.
        let dim = self
            .cells
            .get(ix)
            .and_then(|cell| cell.dim)
            .unwrap_or_else(|| self.dim_target(ix));
        // The mirror shows what its cover shows: the real disc face once
        // that bake has finished (read-only here, the cover above claims
        // the bakes), the style's own blank plate short of that, same as
        // the cover reads it. Nothing to paint at all only when there's
        // neither a disc bake nor a thumb.
        let disc_on = self.config.disc_style != DiscStyle::Off;
        let baked = if disc_on {
            self.discs
                .ready(&path)
                .or_else(|| discs::blank_disc(self.config.disc_style))
        } else {
            None
        };
        if baked.is_none() && image.is_none() {
            return None;
        }
        // The cover's quad, or its flat rect used as one: either way the
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
        // far corners are past zero so the ramp ends there, and the
        // shader clamps the rest of the face to nothing.
        let spent = 1.0 - 1.0 / REFL;
        let desaturated = self.desaturated(ix);
        // Mirrors are see-through, so overlapping ones double-expose
        // instead of occluding. The floor has to tile the way the shelf
        // does: each mirror clips to the slice of its cover the nearer
        // neighbor toward the center leaves visible, and the seams fall
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
        // orientation (the flip happens in the canvas' texture sampling),
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
                        wash,
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

    /// The hero's caption: album over artist, centered. `below` is where
    /// the Center position hangs it, right under the cover's lower edge;
    /// `rail` lifts the Bottom position clear of a horizontal letter rail.
    /// Hidden never gets here; the caller skips the child.
    fn label(&self, ix: usize, pos: LabelPos, below: f32, rail: bool, cx: &App) -> Div {
        let (album, album_reading, artist, artist_reading) = {
            let library = self.state.library.read(cx);
            match (self.cells.get(ix), library.projection()) {
                (Some(cell), Some(projection)) => self
                    .view
                    .get(cell.start)
                    .map(|&row| {
                        let v = projection.resolve(row);
                        // Rows from before the album artist column have an
                        // empty one; the first track's artist stands in,
                        // with its own sort name for the reading.
                        let (artist, artist_sort) = if v.album_artist.is_empty() {
                            (v.artist, v.artist_sort)
                        } else {
                            (v.album_artist, v.album_artist_sort)
                        };
                        (
                            SharedString::from(v.album.to_string()),
                            SharedString::from(v.album_sort.to_string()),
                            SharedString::from(artist.to_string()),
                            SharedString::from(artist_sort.to_string()),
                        )
                    })
                    .unwrap_or_default(),
                _ => Default::default(),
            }
        };
        let readings = crate::settings::show_readings();
        let has_text = !album.is_empty() || !artist.is_empty();
        let anchor = div().absolute().left_0().right_0();
        let anchor = match pos {
            // The rail owns whichever edge it's down on.
            LabelPos::Top => anchor.top(px(if rail { 22. } else { 6. })),
            LabelPos::Center => anchor.top(px(below)),
            LabelPos::Bottom | LabelPos::Hidden => anchor.bottom(px(if rail { 22. } else { 6. })),
        };
        anchor.flex().flex_col().items_center().when(has_text, |d| {
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
                                .child(panel::named(&album, &album_reading, readings)),
                        )
                    })
                    .when(!artist.is_empty(), |d| {
                        d.child(
                            div()
                                .max_w(relative(1.0))
                                .truncate()
                                .text_xs()
                                .text_color(palette::text_secondary())
                                .child(panel::named(&artist, &artist_reading, readings)),
                        )
                    }),
            )
        })
    }

    /// Solo or popped out there's no title bar to host the search, so it
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

    /// The letter rail along the shelf's edge: each initial once, a click
    /// jumping the carousel to its first album. Along the bottom for a
    /// row, down the right for a column, unless `letters_side` swaps it
    /// to the near edge instead.
    fn letter_rail(&self, axis: Axis, cx: &mut Context<Self>) -> Option<Div> {
        if !self.config.letters {
            return None;
        }
        // The lit letter: the last rail entry at or before the center.
        let center = self.pos.round().max(0.) as usize;
        let active = self
            .letters
            .iter()
            .rposition(|&(_, ix)| ix <= center)
            .unwrap_or(0);
        let start = self.config.letters_side == LetterSide::Start;
        let rail = panel::letter_rail(
            &self.letters,
            active,
            axis == Axis::Horizontal,
            self.config.letters_compact,
            |this: &mut Self, first, cx| {
                this.touch_resume(cx);
                this.navigate(first, cx);
            },
            cx,
        )?;
        // The strip positions itself only along its axis; the shelf hangs
        // it on the bottom edge for a row, the right edge for a column,
        // or the near edge of either when swapped.
        Some(if axis == Axis::Horizontal {
            let rail = div().absolute().left_0().right_0().child(rail);
            if start {
                rail.top(tokens::SPACE_XS)
            } else {
                rail.bottom(tokens::SPACE_XS)
            }
        } else {
            let rail = div().absolute().top_0().bottom_0().child(rail);
            if start {
                rail.left(tokens::SPACE_XS)
            } else {
                rail.right(tokens::SPACE_XS)
            }
        })
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
                    rox_i18n::t!("art-layout-section"),
                    None,
                    div()
                        .flex()
                        .flex_col()
                        .gap(tokens::SPACE_MD)
                        .child(setting_row(
                            rox_i18n::t!("art-vertical-layout"),
                            Some(rox_i18n::t!("art-vertical-layout.description")),
                            toggle(
                                self.config.vertical,
                                |this: &mut Self, on, cx| {
                                    this.set_orientation(on, cx);
                                },
                                cx,
                            ),
                        )),
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
                    rox_i18n::t!("art-follow-description"),
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
                    rox_i18n::t!("art-resume-description"),
                    |this: &mut Self, on, cx| {
                        this.config.resume_playing = on;
                        cx.notify();
                    },
                    self.config.smooth_follow,
                    rox_i18n::t!("art-smooth-description"),
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
                            rox_i18n::t!("grid-dim-while-playing"),
                            Some(rox_i18n::t!("art-dim-while-playing")),
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
                                rox_i18n::t!("grid-dim-amount"),
                                Some(rox_i18n::t!("grid-dim-amount.description")),
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
                            rox_i18n::t!("grid-desaturate"),
                            Some(rox_i18n::t!("art-desaturate")),
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
                                    rox_i18n::t!("grid-always"),
                                    Some(rox_i18n::t!("art-always")),
                                    toggle(
                                        self.config.dim_always,
                                        |this: &mut Self, on, cx| {
                                            this.config.dim_always = on;
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

    /// The shelf's own appearance row on the shared page: the covers'
    /// rounding, a look knob stored on the config rather than the theme
    /// because it shapes the covers, not the panel frame.
    fn appearance(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rounding = self.config.rounding;
        Some(
            settings_ui::section(
                rox_i18n::t!("art-covers-section"),
                None,
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(setting_row(
                        rox_i18n::t!("art-perspective"),
                        Some(rox_i18n::t!("art-perspective.description")),
                        toggle(
                            self.config.perspective,
                            |this: &mut Self, on, cx| {
                                this.config.perspective = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    // Nothing turns without the projection, so the angle
                    // has nothing to say; the toggle's own description is
                    // where that's spelled out.
                    .when(self.config.perspective, |page| {
                        page.child(setting_row(
                            rox_i18n::t!("art-tilt"),
                            Some(rox_i18n::t!("art-tilt.description")),
                            settings_ui::scalar(
                                &self.tilt_scrub,
                                &self.value_edit,
                                self.config.tilt,
                                settings_ui::span(-TILT_MAX, TILT_MAX, "°").hard(),
                                |this: &mut Self, value, cx| {
                                    this.config.tilt = value;
                                    cx.notify();
                                },
                                cx,
                            ),
                        ))
                    })
                    // Sizing sits with the rest of the shelf's geometry
                    // rather than off on the behavior page: how big the
                    // hero gets, then how the rack around it is spaced and
                    // counted.
                    .child(setting_row(
                        rox_i18n::t!("art-fill-panel"),
                        Some(rox_i18n::t!("art-fill-panel.description")),
                        toggle(
                            self.config.fill,
                            |this: &mut Self, on, cx| {
                                this.config.fill = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        rox_i18n::t!("art-spacing"),
                        Some(rox_i18n::t!("art-spacing.description")),
                        settings_ui::scalar(
                            &self.spacing_scrub,
                            &self.value_edit,
                            self.config.spacing,
                            settings_ui::span(SHIFT_MIN, SHIFT_MAX, "%").hard(),
                            |this: &mut Self, value, cx| {
                                this.config.spacing = value;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        rox_i18n::t!("art-stride"),
                        Some(rox_i18n::t!("art-stride.description")),
                        settings_ui::scalar(
                            &self.stride_scrub,
                            &self.value_edit,
                            self.config.stride,
                            settings_ui::span(STEP_MIN, STEP_MAX, "%").hard(),
                            |this: &mut Self, value, cx| {
                                this.config.stride = value;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        rox_i18n::t!("art-recede"),
                        Some(rox_i18n::t!("art-recede.description")),
                        settings_ui::scalar(
                            &self.recede_scrub,
                            &self.value_edit,
                            self.config.recede,
                            settings_ui::span(0., 100., "%").hard(),
                            |this: &mut Self, value, cx| {
                                this.config.recede = value;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        rox_i18n::t!("art-visible"),
                        Some(rox_i18n::t!("art-visible.description")),
                        settings_ui::scalar(
                            &self.visible_scrub,
                            &self.value_edit,
                            self.config.visible as f32,
                            settings_ui::span(1., VIS_MAX, "").hard(),
                            |this: &mut Self, value, cx| {
                                this.config.visible = value.round().clamp(1., VIS_MAX) as u8;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child({
                        // DISC_STYLES holds i18n keys, not labels; choices_shared
                        // wants the resolved text, so translate before handing it
                        // off rather than through the legacy `choices` adapter.
                        let styles: Vec<_> = discs::DISC_STYLES
                            .iter()
                            .map(|(key, style)| (rox_i18n::t!(*key), *style))
                            .collect();
                        setting_row(
                            rox_i18n::t!("art-disc-style"),
                            Some(rox_i18n::t!("art-disc-style.description")),
                            panel::choices_shared(
                                &styles,
                                self.config.disc_style,
                                |this: &mut Self, style, cx| this.set_disc_style(style, cx),
                                cx,
                            ),
                        )
                    })
                    // A disc is already round and a keystone can't clip
                    // rounded, so the knob only shows while the covers
                    // paint flat and square.
                    .when(
                        self.config.disc_style == DiscStyle::Off && !self.config.perspective,
                        |page| {
                            page.child(setting_row(
                                rox_i18n::t!("library-art-rounding"),
                                Some(rox_i18n::t!("grid-art-rounding-description")),
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
                        },
                    )
                    .child(setting_row(
                        rox_i18n::t!("art-reflections"),
                        Some(rox_i18n::t!("art-reflections.description")),
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
                        rox_i18n::t!("art-shadows"),
                        Some(rox_i18n::t!("art-shadows.description")),
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
                        rox_i18n::t!("art-glow"),
                        Some(rox_i18n::t!("art-glow.description")),
                        toggle(
                            self.config.glow,
                            |this: &mut Self, on, cx| {
                                this.config.glow = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        rox_i18n::t!("art-label-position"),
                        Some(rox_i18n::t!("art-label-position.description")),
                        panel::choices_shared(
                            &[
                                (rox_i18n::t!("valign-top"), LabelPos::Top),
                                (rox_i18n::t!("valign-middle"), LabelPos::Center),
                                (rox_i18n::t!("valign-bottom"), LabelPos::Bottom),
                                (rox_i18n::t!("arrange-hidden"), LabelPos::Hidden),
                            ],
                            self.config.label,
                            |this: &mut Self, pos, cx| {
                                this.config.label = pos;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(setting_row(
                        rox_i18n::t!("art-letter-rail"),
                        Some(rox_i18n::t!("art-letter-rail.description")),
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
                    }),
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

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("art-title"),
        )
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
    /// panel's body right-click stays out; the panel dropdown is appended
    /// after the play items, the library's arrangement.
    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

    /// The layout dump stores the panel's config; the builder registered in
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
                PopupMenuItem::new(rox_i18n::t!("panel-jump-to-playing"))
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

        // Display section: the view knobs group under flyouts so the menu
        // stays short, the same shape as the library's.
        let menu = menu.separator().label(rox_i18n::t!("panel-menu-display"));
        // The scroll direction, a checked pair so the current axis reads at
        // a glance.
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
        // The letter rail, icon on the row so the tick lands on the right
        // like every other top-level check row.
        let menu = menu.item(panel::check_row(
            rox_i18n::t!("art-letter-rail"),
            Some(icons::PANEL_RIGHT),
            |this: &Self| this.config.letters,
            |this, cx| {
                this.config.letters = !this.config.letters;
                cx.notify();
            },
            &cx.entity(),
        ));
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
        // is applied here, where a window exists to set the input's text.
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
                // on, and publish the album it settled on.
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
        // step here; the rest of the shelf stays frozen off-screen, where its
        // opacity doesn't show, until it scrolls back on. A big library's
        // off-shelf covers cost nothing. Frames only while one is moving.
        self.warm_discs(cx);

        let dim_step = 1.0 - (0.08_f32).powf(dt * 10.0);
        let last = self.cells.len().saturating_sub(1) as i64;
        let lo = (self.pos.floor() as i64 - self.visible()).clamp(0, last);
        let hi = (self.pos.ceil() as i64 + self.visible()).clamp(0, last);
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

        // The search shows in the tab bar via title_suffix while the panel
        // shares a group; solo or popped out there's no header at all, so it
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
            // Bindings win over key listeners and an action stops
            // propagation by default, so the workspace's left and right
            // would seek instead of ever reaching the listener below.
            // PanelNav takes the pair back while the shelf holds focus; the
            // type-ahead pair joins it while a phrase is up, to take back
            // space (only while the phrase is still absorbing keystrokes)
            // and tab (for as long as there's a phrase to cycle).
            .key_context(panel::panel_nav_context(
                &self.type_ahead,
                self.type_ahead_at,
            ))
            // Tab cycles the live phrase's matches, off the bindings the
            // TypeAhead context above scopes in; with no phrase up, tab
            // stays Root's focus traversal.
            .on_action(cx.listener(|this, _: &TypeAheadNext, _, cx| this.type_step(false, cx)))
            .on_action(cx.listener(|this, _: &TypeAheadPrev, _, cx| this.type_step(true, cx)))
            // Arrow browsing while the shelf itself holds focus. The guard
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
        let content: AnyElement = if self.cells.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(palette::text_muted())
                .child(
                    if self.effective_query(cx).is_empty() && self.effective_filter(cx).is_empty() {
                        rox_i18n::t!("grid-library-empty")
                    } else {
                        rox_i18n::t!("picker-no-matches")
                    },
                )
                .into_any_element()
        } else {
            let (w, h) = self.frame();
            let hero = self.hero_side();
            let axis = self.axis();
            // The scroll axis takes the stacking offset; the other keeps
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
            let lo = (self.pos.floor() as i64 - self.visible()).max(0);
            let hi = (self.pos.ceil() as i64 + self.visible()).min(self.cells.len() as i64 - 1);
            let mut order: Vec<i64> = (lo..=hi).collect();
            order.sort_by(|a, b| {
                let da = (*b as f32 - self.pos).abs();
                let db = (*a as f32 - self.pos).abs();
                da.partial_cmp(&db).unwrap()
            });
            // The pointer map for this frame, nearest cover first, the paint
            // order read backwards. A cover on its way out of the window is
            // too faint to aim at, so it drops out of the map before it
            // stops painting.
            self.hits = order
                .iter()
                .rev()
                .filter_map(|&ix| {
                    let d = ix as f32 - self.pos;
                    (self.edge_fade(d.abs()) >= HIT_OP)
                        .then(|| (ix as usize, self.outline(d, hero, cx_px, cy_px)))
                })
                .collect();

            // The id is what buys the shelf its hover state: leaving it is
            // the only way the pointer stops being over a cover without a
            // move event to say so.
            let mut shelf = div()
                .id("art-shelf")
                .relative()
                .flex_1()
                .min_h_0()
                .overflow_hidden();
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
            // A horizontal rail sits on the shelf's top or bottom edge, so
            // a label sharing that edge lifts clear of it.
            let rail_active =
                self.config.letters && self.letters.len() >= 2 && axis == Axis::Horizontal;
            let rail_start = self.config.letters_side == LetterSide::Start;
            let rail_edge = match self.config.label {
                LabelPos::Top => rail_active && rail_start,
                LabelPos::Bottom | LabelPos::Hidden => rail_active && !rail_start,
                LabelPos::Center => false,
            };
            let shelf = if self.config.label == LabelPos::Hidden {
                shelf
            } else {
                shelf.child(self.label(
                    center,
                    self.config.label,
                    cy_px + hero / 2.0 + 8.,
                    rail_edge,
                    cx,
                ))
            };
            shelf
                // Any press might be a scrub; the covers' clicks moved to
                // release so both can tell.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        // A press has moved the cursor by hand, so any
                        // cycle the phrase was stepping is stale.
                        this.clear_type_ahead(cx);
                        this.flick.begin(event.position.along(axis));
                        this.coasting = true;
                        this.publish_pending = true;
                        this.touch_resume(cx);
                        cx.notify();
                    }),
                )
                // The cover click, resolved against the outlines: a press
                // that traveled was a scrub, and a press on the floor
                // between two covers is neither cover's.
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseUpEvent, window, cx| {
                        if this.flick.scrolled() {
                            return;
                        }
                        let Some(ix) = this.hit(event.position) else {
                            return;
                        };
                        this.focus.focus(window);
                        if event.click_count > 1 {
                            this.play(ix, cx);
                            this.navigate(ix, cx);
                        } else {
                            // Center it and select it; the settle
                            // republishes, this makes the click feel
                            // immediate.
                            this.select_only(ix, cx);
                            this.navigate(ix, cx);
                        }
                    }),
                )
                // Hover lights a cover back up under the focus effects and
                // aims the context menu, so it follows the same outlines.
                .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                    let target = this.hit(event.position);
                    if this.hovered != target {
                        this.hovered = target;
                        cx.notify();
                    }
                }))
                // Off the shelf entirely there's no move event to clear it.
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if !*hovered && this.hovered.is_some() {
                        this.hovered = None;
                        cx.notify();
                    }
                }))
                // The wheel steps the carousel a cover at a time, banking the
                // sub-step travel so a trackpad's small deltas still count.
                // The scroll axis leads; a plain vertical wheel still drives
                // a row when that's all the mouse sends.
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
                                        // The origin rides along: the
                                        // pointer arrives in window space
                                        // and the outlines are in the
                                        // shelf's. No notify on a move
                                        // alone; nothing about the layout
                                        // changed.
                                        this.origin = bounds.origin;
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
                .children(self.letter_rail(axis, cx))
                .children(panel::type_ahead_overlay(
                    &self.type_ahead,
                    self.type_ahead_at,
                ))
                // The shelf's right-click menu, keyed off the hovered cover
                // since the builder gets no position: the hovered cover is
                // selected first so the menu acts on what's highlighted. Off
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
                            PopupMenuItem::new(rox_i18n::t!("library-play"))
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
                        let convert_state = state.clone();
                        let convert_ids = ids.clone();
                        let copy_ids = ids.clone();
                        let menu = menu.item(
                            PopupMenuItem::new(rox_i18n::t!("art-edit-tags"))
                                .icon(Icon::default().path(icons::PENCIL))
                                .on_click(move |_, _, cx| {
                                    rox_panel_api::openers::tags_editor(
                                        state.clone(),
                                        ids.clone(),
                                        cx,
                                    );
                                }),
                        );
                        // The whole album out to another format, the shelf's
                        // own door to it: this menu is built here rather than
                        // through `track_actions`, so the row has to be added
                        // twice. Gated on ffmpeg being installed, same as the
                        // track menu's.
                        let menu = if rox_panel_api::openers::convert_available() {
                            menu.item(
                                PopupMenuItem::new(rox_i18n::t!("art-convert"))
                                    .icon(Icon::default().path(icons::AUDIO_LINES))
                                    .on_click(move |_, _, cx| {
                                        rox_panel_api::openers::convert_dialog(
                                            convert_state.clone(),
                                            convert_ids.clone(),
                                            cx,
                                        );
                                    }),
                            )
                        } else {
                            menu
                        };
                        // Copy takes the album's tracks, one line each.
                        let menu = panel::copy_ids_submenu(
                            menu,
                            this.read(cx).state.clone(),
                            copy_ids,
                            window,
                            cx,
                        );
                        // Reveal follows the album's first track, opening
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
                                    PopupMenuItem::new(rox_i18n::t!("library-filter-by-artist"))
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
