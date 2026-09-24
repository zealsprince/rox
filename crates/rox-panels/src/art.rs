//! The art view panel: the catalog as a cover carousel, NekoRoX's shelf.
//! One album centered and square, its neighbors shrinking, turning, and
//! tucking behind it toward both edges. A row by default, or a column by a
//! setting. The turn is a real projection through the sprite pipeline,
//! since gpui has no 3D of its own. It shares the album grid's model: one
//! entry per album in canonical order, textures through the shared artwork
//! service, the same search, follow-playing, dim, and play rules. Per the
//! workspace rule, a browsing surface is a panel of its own, never a
//! library view mode.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    Along, AnyElement, App, Axis, Bounds, BoxShadow, Context, Div, Entity, EventEmitter,
    FocusHandle, Focusable, ImageSource, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ObjectFit, Pixels, RenderImage, ScrollWheelEvent, SharedString, Size,
    Subscription, WeakEntity, Window, canvas, div, hsla, img, point, prelude::*, px, relative,
    size, svg,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Side};
use image::Frame;
use rox_core::QUEUE_CAP;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::TrackKey;
use rox_library::projection::{Projection, QUERY_FIELDS, QueryField};
use rox_panel_api::actions::{TypeAheadNext, TypeAheadPrev};
use rox_panel_kit::config::{default_true, is_zero};
use rox_panel_kit::wall::{TILE_DIM_MAX, default_dim};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::discs::{self, DiscCache, DiscShape, DiscStyle};
use crate::grid::LetterSide;
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

const TILE_ROUNDING_MAX: f32 = 100.;

/// Covers drawn to each side of the centered one, and the setting's ceiling.
const VIS: u8 = 5;
const VIS_MAX: f32 = 16.;

/// Scale per step out from center, floored at [`MIN_SCALE`].
const SHRINK: f32 = 0.86;
const MIN_SCALE: f32 = 0.5;

/// The first flank's center, in percent of the hero's edge. Under 100 the
/// neighbor tucks behind the hero.
const SHIFT0: f32 = 56.;
const SHIFT_MIN: f32 = 20.;
const SHIFT_MAX: f32 = 140.;
/// Each further cover's step past the first, same units. Also the drag
/// mapping: how far the shelf travels per cover.
const STEP: f32 = 30.;
const STEP_MIN: f32 = 5.;
const STEP_MAX: f32 = 100.;

/// How dark a fully turned cover's shading goes, as a fraction of the
/// panel's background.
const TURN_SCRIM: f32 = 90. / 255.;

/// The deepest cover's brightness, in percent. The covers between share the
/// distance evenly.
const RECEDE: f32 = 20.;

/// How much of its step the last cover spends fading out, so covers arrive
/// and leave instead of popping.
const EDGE_FADE: f32 = 1.0;
/// Below this opacity a cover stops taking clicks and hovers.
const HIT_OP: f32 = 0.08;

/// Covers past the shelf's depth whose artwork and disc bake start early.
const WARM: i64 = 4;

const LABEL_H: f32 = 40.;

/// The mirror's length past a cover's lower edge, as a fraction of the
/// cover, and its starting opacity.
const REFL: f32 = 0.32;
const REFL_OP: f32 = 0.45;
const REFL_GAP: f32 = 2.;

/// Flank turn in degrees, its ceiling, and the projection's focal length in
/// hero edges. Past 85 a cover is edge-on. Negative turns the rack outward.
const TILT: f32 = 55.;
const TILT_MAX: f32 = 85.;
const FOCAL: f32 = 2.8;

/// Wheel travel, in px, that advances the carousel by one cover.
const WHEEL_STEP: f32 = 40.;

const FALLBACK_W: f32 = 600.;
const FALLBACK_H: f32 = 320.;

#[derive(Clone, Serialize, Deserialize)]
pub struct ArtConfig {
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
    pub vertical: bool,
    /// Size the hero off the cross axis alone, letting the flanks run off
    /// the panel edge.
    #[serde(default)]
    pub fill: bool,
    #[serde(default)]
    pub follow_playing: bool,
    /// Glide the playing album back to center after the shelf sits idle.
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
    /// How far the dimmed covers fade, in percent of fully hidden.
    #[serde(default = "default_dim")]
    pub dim: f32,
    /// Corner rounding, in percent of circular.
    #[serde(default)]
    pub rounding: f32,
    #[serde(default = "default_true")]
    pub reflection: bool,
    #[serde(default)]
    pub shadow: bool,
    #[serde(default)]
    pub glow: bool,
    /// The rounding knob stands down while a style is on.
    #[serde(default)]
    pub disc_style: DiscStyle,
    /// Art rounding only applies with this off.
    #[serde(default = "default_true")]
    pub perspective: bool,
    /// Flank turn, in degrees.
    #[serde(default = "default_tilt")]
    pub tilt: f32,
    /// The first flank's distance from the hero, in percent of its edge.
    #[serde(default = "default_spacing")]
    pub spacing: f32,
    /// The gap between covers past the first flank, in percent of the hero's
    /// edge. Also the drag mapping.
    #[serde(default = "default_stride")]
    pub stride: f32,
    #[serde(default = "default_visible")]
    pub visible: u8,
    /// Painted as a wash toward the background, not transparency, since
    /// see-through covers would show the whole overlapping stack.
    #[serde(default = "default_recede")]
    pub recede: f32,
    #[serde(default)]
    pub letters: bool,
    #[serde(default)]
    pub letters_compact: bool,
    #[serde(default)]
    pub letters_side: LetterSide,
    #[serde(default)]
    pub label: LabelPos,
    /// The centered cell index at save time, restored on relaunch.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub center: usize,
}

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

/// One album's run in the current view. `art` resolves on first paint; the
/// inner None is a track the store no longer knows.
struct Cell {
    start: usize,
    len: u32,
    art: Option<Option<PathBuf>>,
    /// Eased opacity under the dim mode. None until first paint, which starts
    /// at the target so only changes fade.
    dim: Option<f32>,
}

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

/// Point in a convex quad, corners in order. A point on an edge counts as
/// in, so a click on the seam between two covers lands.
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

/// The centered square as a fractional source rect for `paint_image_quad`,
/// matching `ObjectFit::Cover`, since thumbs aren't baked square.
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

struct Placement {
    left: f32,
    top: f32,
    w: f32,
    h: f32,
    /// The window-edge fade, before the dim mode multiplies in.
    fade: f32,
    recede: f32,
    /// 0 at the hero, 1 a full step out.
    turn: f32,
}

pub struct ArtPanel {
    state: AppState,
    config: ArtConfig,
    /// The canonical order while the query is empty, otherwise the hits
    /// re-sorted canonically so an album stays one contiguous run.
    view: Arc<Vec<u32>>,
    cells: Vec<Cell>,
    /// Cells still easing their dim, so a frame doesn't scan every cover.
    dimming: HashSet<usize>,
    /// Each distinct initial and its first cell.
    letters: Vec<(SharedString, usize)>,
    discs: DiscCache,
    search: Entity<SearchBox>,
    /// A set of one, so the grid's context menu and publish path apply as is.
    selected: HashSet<usize>,
    hovered: Option<usize>,
    /// The animated center, fractional while a move is in flight.
    pos: f32,
    goal: f32,
    /// True while a drag or its coast owns `pos`. Clears once the ease takes
    /// over.
    coasting: bool,
    /// Set by a user move, spent on the next settle so a scrub publishes only
    /// the album it lands on.
    publish_pending: bool,
    wheel: f32,
    /// The dock caches panels, so a resize repaints without re-rendering. A
    /// measuring canvas compares against this and notifies on drift.
    size: Size<Pixels>,
    origin: gpui::Point<Pixels>,
    /// Painted cover outlines in shelf space, nearest first. Clicks test the
    /// shape, since a turned cover's box holds floor it doesn't cover.
    hits: Vec<(usize, [[f32; 2]; 4])>,
    flick: FlickState,
    last_tick: Instant,
    resume_idle: ResumeIdle,
    playing_key: Option<TrackKey>,
    /// Kept fresh by `sync_playing` and `rebuild` so per-frame dimming never
    /// rescans.
    playing_ix: Option<usize>,
    playing: bool,
    /// The last published centered cell, so a settle republishes only on
    /// change.
    centered: Option<usize>,
    rounding_scrub: ScrubState,
    dim_scrub: ScrubState,
    tilt_scrub: ScrubState,
    spacing_scrub: ScrubState,
    stride_scrub: ScrubState,
    visible_scrub: ScrubState,
    recede_scrub: ScrubState,
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
    _search_events: Subscription,
    _query_changed: Subscription,
    _selection_changed: Subscription,
    _player_changed: Subscription,
    /// Drops the phrase on blur so tab goes back to walking panels.
    _type_ahead_blur: Subscription,
}

impl ArtPanel {
    pub fn new(
        state: AppState,
        config: ArtConfig,
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
        let initial = match config.query_source {
            QuerySource::Global => state.query.read(cx).text().to_string(),
            QuerySource::Local | QuerySource::Selection => config.query.clone(),
        };
        let search =
            cx.new(|cx| SearchBox::new(rox_i18n::t!("query-search"), &initial, window, cx).small());
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
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
        // Follow-playing owns the center on launch, so it skips the saved one.
        let start = if config.follow_playing {
            0
        } else {
            config.center
        } as f32;
        let focus = cx.focus_handle().tab_stop(true);
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
            // Suppress the launch settle's publish: a restore doesn't
            // reselect.
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

    /// The automatic follow never touches the selection.
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

    /// Unlike the automatic follow, the menu's jump publishes the selection.
    fn jump_to_playing(&mut self, cx: &mut Context<Self>) {
        let Some(cell_ix) = self.playing_ix else {
            return;
        };
        self.select_only(cell_ix, cx);
        self.follow_playing(cx);
    }

    fn touch_resume(&mut self, cx: &mut Context<Self>) {
        if self.config.resume_playing {
            self.resume_idle.touch(cx, Self::resume_to_playing);
        }
    }

    /// The clock only fires after a full untouched window, so no idle check
    /// here.
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

    fn navigate(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.goal = ix as f32;
        self.coasting = false;
        self.publish_pending = true;
        cx.notify();
    }

    /// Measured off `goal` rather than `pos`, so a held arrow banks its steps
    /// instead of fighting the ease.
    fn step_cover(&mut self, delta: i64, cx: &mut Context<Self>) {
        let last = self.max_index() as i64;
        let target = (self.goal.round() as i64 + delta).clamp(0, last) as usize;
        self.center_on(target, cx);
    }

    fn center_on(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.cells.len() {
            return;
        }
        self.select_only(ix, cx);
        self.navigate(ix, cx);
    }

    /// Modifiers pass through so the workspace keeps its shortcuts.
    fn on_panel_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.alt {
            return;
        }
        self.touch_resume(cx);
        match keystroke.key.as_str() {
            "left" | "up" => self.step_cover(-1, cx),
            "right" | "down" => self.step_cover(1, cx),
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
                // Stop here so space doesn't also fire the workspace's
                // TogglePlayback binding.
                cx.stop_propagation();
                self.type_to(text.clone(), cx);
            }
        }
    }

    /// Split a leading `field:` pin off the phrase. True means it pins the
    /// artist, false the album; other fields read literally.
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

    /// A fresh phrase starts past the centered cover, so the same letter
    /// steps to the next match. A grown one re-tests it so refining stays put.
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
        let pin = Self::type_ahead_pin(&needle);
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

    /// Drops the measured frame so the next paint re-measures the new cross
    /// axis.
    fn set_orientation(&mut self, vertical: bool, cx: &mut Context<Self>) {
        if self.config.vertical == vertical {
            return;
        }
        self.config.vertical = vertical;
        self.size = Size::default();
        cx.notify();
    }

    /// Hits filter the canonical order rather than being iterated, or an
    /// album's scattered rows would split into duplicate covers. Breaks on
    /// the album artist so a compilation stays one cover.
    fn rebuild(&mut self, cx: &mut Context<Self>) {
        self.cells.clear();
        self.dimming.clear();
        self.selected.clear();
        // The same index can now be a different album, so a settle there
        // must publish.
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
        self.letters.clear();
        if let Some(projection) = self.state.library.read(cx).projection() {
            for (ix, cell) in self.cells.iter().enumerate() {
                let row = self.view[cell.start] as usize;
                // The ordering's key, so a sort-tagged name lands under the
                // rail's letter.
                let name = projection
                    .album_artists
                    .sort_key(projection.album_artist[row] as usize);
                let letter = panel::letter_initial(name);
                if self.letters.last().map(|(l, _)| l.as_ref()) != Some(letter.as_str()) {
                    self.letters.push((SharedString::from(letter), ix));
                }
            }
        }
        // Only clamp once the projection is loaded, or a cold start pins the
        // restored center to 0.
        if has_projection {
            let max = self.max_index();
            self.pos = self.pos.clamp(0., max);
            self.goal = self.goal.clamp(0., max);
        }
        self.playing_ix = self.playing_cell(cx);
        cx.notify();
    }

    fn max_index(&self) -> f32 {
        self.cells.len().saturating_sub(1) as f32
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

    /// The first track's artist stands in for the album's.
    fn cell_artist(&self, ix: usize, cx: &App) -> Option<String> {
        let cell = self.cells.get(ix)?;
        let row = *self.view.get(cell.start)?;
        let library = self.state.library.read(cx);
        let projection = library.projection()?;
        Some(projection.resolve(row).artist.to_string())
    }

    fn art_path(&mut self, ix: usize, cx: &Context<Self>) -> Option<PathBuf> {
        if let Some(art) = self.cells.get(ix).and_then(|cell| cell.art.clone()) {
            return art;
        }
        let path = {
            let library = self.state.library.read(cx);
            let id = self.cells.get(ix).and_then(|cell| {
                let projection = library.projection()?;
                let row = *self.view.get(cell.start)?;
                // The untagged bucket keeps the placeholder instead of a
                // loose track's art.
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

    fn select_only(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = HashSet::from([ix]);
        self.centered = Some(ix);
        self.publish_selection(cx);
        cx.notify();
    }

    fn publish_selection(&mut self, cx: &mut Context<Self>) {
        let mut ixs: Vec<usize> = self.selected.iter().copied().collect();
        ixs.sort_unstable();
        let ids: Vec<i64> = ixs.iter().flat_map(|&ix| self.ids_for(ix, cx)).collect();
        let source = cx.entity_id();
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, source, cx));
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

    fn axis(&self) -> Axis {
        if self.config.vertical {
            Axis::Vertical
        } else {
            Axis::Horizontal
        }
    }

    fn frame(&self) -> (f32, f32) {
        let w = f32::from(self.size.width);
        let h = f32::from(self.size.height);
        if w <= 0. || h <= 0. {
            (FALLBACK_W, FALLBACK_H)
        } else {
            (w, h)
        }
    }

    fn hero_side(&self) -> f32 {
        let (w, h) = self.frame();
        let avail_h = h - LABEL_H;
        // A row mirrors one floor below, a column both side edges.
        let (floor, sides) = if self.config.reflection {
            (1.0 + REFL, 1.0 + 2.0 * REFL)
        } else {
            (1.0, 1.0)
        };
        // Fit keeps the first flanks inside the panel, trading hero size for
        // wider spacing. Fill spends the whole axis and lets the flanks clip.
        let cap = if self.config.fill {
            1.0
        } else {
            (0.42 * SHIFT0 / self.config.spacing.max(1.)).min(1.0)
        };
        match self.axis() {
            Axis::Horizontal => (avail_h * 0.9 / floor).min(w * cap),
            Axis::Vertical => (w * 0.86 / sides).min(avail_h * cap),
        }
        .max(48.)
    }

    fn step_px(&self) -> f32 {
        (self.hero_side() * self.config.stride / 100.).max(1.)
    }

    fn visible(&self) -> i64 {
        self.config.visible.max(1) as i64
    }

    fn receded(&self, ix: usize) -> bool {
        if self.hovered == Some(ix) || self.playing_ix == Some(ix) {
            return false;
        }
        self.config.dim_always || self.playing
    }

    fn dim_target(&self, ix: usize) -> f32 {
        if self.config.dim_playing && self.receded(ix) {
            1.0 - self.config.dim / TILE_DIM_MAX
        } else {
            1.0
        }
    }

    fn desaturated(&self, ix: usize) -> bool {
        self.config.desaturate_playing && self.receded(ix)
    }

    /// A hit returns at once. A miss claims one off-thread bake and returns
    /// None until it lands.
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

    /// The quad arrives relative to the canvas' top-left, and the paint hook
    /// re-anchors it to wherever the div ends up.
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
                // corners, so the vertex map stays orientation-true.
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

    /// Bake a few covers past the shelf, so a scroll doesn't show flat art
    /// popping into discs.
    fn warm_discs(&mut self, cx: &mut Context<Self>) {
        if self.config.disc_style == DiscStyle::Off {
            return;
        }
        let last = self.cells.len().saturating_sub(1) as i64;
        let reach = self.visible() + WARM;
        let center = self.pos.round() as i64;
        let lo = (center - reach).clamp(0, last);
        let hi = (center + reach).clamp(0, last);
        // Outward from the center, so a full pool spends its slots on the
        // nearest covers.
        let mut window: Vec<i64> = (lo..=hi).collect();
        window.sort_by_key(|ix| (ix - center).abs());
        for ix in window {
            let ix = ix as usize;
            let Some(path) = self.art_path(ix, cx) else {
                continue;
            };
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

    /// Bakes are per style, so a change drops the cache.
    fn set_disc_style(&mut self, style: DiscStyle, cx: &mut Context<Self>) {
        if self.config.disc_style != style {
            self.config.disc_style = style;
            self.discs.clear();
        }
        cx.notify();
    }

    /// The only real transparency a cover gets. The window runs a cover
    /// deeper than the count to make room for the ramp.
    fn edge_fade(&self, a: f32) -> f32 {
        let depth = self.visible() as f32;
        ((depth + EDGE_FADE - a) / EDGE_FADE).clamp(0., 1.)
    }

    /// 0 at the center to 1 gone. The deepest cover lands on the setting
    /// however deep the rack runs.
    fn recede(&self, a: f32) -> f32 {
        let depth = self.visible() as f32;
        let back = (self.config.recede / 100.).clamp(0., 1.);
        1.0 - (1.0 - (1.0 - back) * (a / depth)).clamp(back, 1.0)
    }

    /// Corners wind the same way for the keystone and the flat rect, so the
    /// containment test doesn't care which it got.
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

    /// `hits` is built nearest-first, so the first match is the cover on top.
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

    /// The cover and its reflection both read this, which keeps the mirror
    /// under its cover through every scrub frame.
    fn placement(&self, d: f32, hero: f32, cx_px: f32, cy_px: f32) -> Placement {
        let a = d.abs();
        let scale = SHRINK.powf(a).max(MIN_SCALE);
        // Only the projection turns a cover. Squashing a flat face reads as
        // squeezed, not turned.
        let turn = if self.config.perspective {
            a.clamp(0., 1.)
        } else {
            0.
        };
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

    /// Corners clockwise from the texture's top-left, in shelf coordinates.
    /// The cover rotates about its cross axis, inner edge toward the viewer.
    fn quad(&self, d: f32, hero: f32, cx_px: f32, cy_px: f32) -> [[f32; 2]; 4] {
        let a = d.abs();
        let scale = SHRINK.powf(a).max(MIN_SCALE);
        let half = hero * scale / 2.0;
        let theta = self.config.tilt.to_radians() * a.clamp(0., 1.) * d.signum();
        let (sin, cos) = theta.sin_cos();
        let focal = hero * FOCAL;
        let off = self.offset_units(d) * hero;
        // An edge at offset `u` along the scroll axis sits at depth u * sin.
        let edge = |u: f32| {
            let s = focal / (focal + u * sin);
            // Cap the near edge to the hero's band so the floor seams hold
            // one clean line.
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

    /// In hero edges. Covers past the first step by a fixed stride, so
    /// widening the hero's gap doesn't pull the stack apart.
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

    /// Pending and missing art share the placeholder, so an arriving cover
    /// fills in without a flash.
    fn cover(
        &mut self,
        ix: usize,
        d: f32,
        hero: f32,
        cx_px: f32,
        cy_px: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
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
        // The turn shading and the depth wash compose as two alphas over the
        // same background, into one coat.
        let scrim = 1.0 - (1.0 - turn * TURN_SCRIM) * (1.0 - recede);
        let wash = palette::alpha(palette::bg_root(), (scrim * 255.) as u8);
        let disc_on = self.config.disc_style != DiscStyle::Off;
        let persp = self.config.perspective;
        // With perspective the div spans the keystone's box and the canvas
        // inside paints the real shape.
        let quad = persp.then(|| self.quad(d, hero, cx_px, cy_px));
        let (left, top, w, h) = match &quad {
            Some(quad) => quad_aabb(quad),
            None => (flat_left, flat_top, flat_w, flat_h),
        };
        // A keystone can't clip rounded. A disc pins the radius to the bake's
        // circle so the ring and shadow follow it.
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
        // Short of its own bake, a disc cover shows the style's shared blank
        // plate, never a flat square.
        let baked = if disc_on {
            match (&thumb, &path) {
                (Thumb::Ready(image), Some(path)) => self.disc_of(path.clone(), image.clone(), cx),
                _ => None,
            }
            .or_else(|| discs::blank_disc(self.config.disc_style))
        } else {
            None
        };
        let fit = ObjectFit::Cover;
        let desaturated = self.desaturated(ix);
        let disc_shown = baked.is_some();
        // The turn and depth wash shade art only. Over the placeholder
        // they'd read as it fading with distance.
        let has_content = disc_shown || matches!(thumb, Thumb::Ready(_));
        let quad_painted = quad.is_some() && has_content;
        let content: AnyElement = match (quad, thumb, baked) {
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
            // The bake is square and carries its own alpha, so Fill needs no
            // clipping.
            (None, _, Some(disc)) => img(disc)
                .size_full()
                .object_fit(ObjectFit::Fill)
                .grayscale(desaturated)
                .into_any_element(),
            (None, Thumb::Ready(image), None) => img(image)
                .size_full()
                // Cover only crops under a mask, or the hero spills onto its
                // neighbors.
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
            // A disc face has its own alpha and a keystone leaves its box's
            // corners bare, so neither gets a backdrop.
            .when(!disc_shown && !quad_painted, |el| {
                el.bg(palette::bg_elevated())
            })
            .opacity(opacity)
            .when(self.config.shadow, |el| {
                el.shadow(vec![BoxShadow {
                    color: hsla(0., 0., 0., 0.35),
                    offset: point(px(0.), px(h * 0.05)),
                    blur_radius: px(h * 0.10),
                    spread_radius: px(0.),
                }])
            })
            // Hover and click are the shelf's: gpui hands a turned cover's
            // whole box the pointer, so the shelf tests outlines instead.
            .cursor_pointer()
            .child(content)
            // The coat the keystone bakes in, as an overlay for the rest.
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

    /// The face flipped past the cover's edge with a true alpha fade, so the
    /// glow shows through. A row gets one floor, a column mirrors both side
    /// edges. Only real art reflects.
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
        // The depth wash but not the turn shading: a mirror is already dim.
        let wash = palette::alpha(palette::bg_root(), (recede * 255.) as u8).into();
        // Read-only: before a first paint the target stands in, so the mirror
        // never races the fade.
        let dim = self
            .cells
            .get(ix)
            .and_then(|cell| cell.dim)
            .unwrap_or_else(|| self.dim_target(ix));
        // Read-only too: the cover claims the bakes.
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
        // The fade runs 1 at the seam to 0 at REFL of the way out, and the
        // shader clamps the rest of the face to nothing.
        let spent = 1.0 - 1.0 / REFL;
        let desaturated = self.desaturated(ix);
        // Mirrors are see-through, so overlapping ones would double-expose.
        // Each clips to the slice its nearer neighbor leaves visible.
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
        // The flip happens in the canvas' sampling, so the mask clips the
        // mirror without touching the mapping.
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
            // Each column reflects across its own bottom edge, so the mirror
            // meets its cover however the keystone leans.
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

    /// An unpainted circle whose blurred shadow is the glow, a cheap radial
    /// gradient.
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

    /// `below` is the Center position's top edge. `rail` lifts the edge
    /// positions clear of a horizontal letter rail.
    fn label(&self, ix: usize, pos: LabelPos, below: f32, rail: bool, cx: &App) -> Div {
        let (album, album_reading, artist, artist_reading) = {
            let library = self.state.library.read(cx);
            match (self.cells.get(ix), library.projection()) {
                (Some(cell), Some(projection)) => self
                    .view
                    .get(cell.start)
                    .map(|&row| {
                        let v = projection.resolve(row);
                        // Rows from before the album artist column have it
                        // empty, so the track artist stands in.
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
            LabelPos::Top => anchor.top(px(if rail { 22. } else { 6. })),
            LabelPos::Center => anchor.top(px(below)),
            LabelPos::Bottom | LabelPos::Hidden => anchor.bottom(px(if rail { 22. } else { 6. })),
        };
        anchor.flex().flex_col().items_center().when(has_text, |d| {
            d.child(
                // The scrim keeps the text readable over the covers a column
                // stacks under the hero.
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

    fn letter_rail(&self, axis: Axis, cx: &mut Context<Self>) -> Option<Div> {
        if !self.config.letters {
            return None;
        }
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
                        // The box keeps its text; the view shows the full
                        // catalog while hidden.
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
                        // DISC_STYLES holds i18n keys, so translate them first.
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
                    // rounded.
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

    /// The shelf serves its own cover menus, so the tab panel's body
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
        // Checks on the right, or the default left side swaps icons out for
        // the checkmark.
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

        let menu = menu.separator().label(rox_i18n::t!("panel-menu-display"));
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
        let menu = crate::query::shared_query::search_flyout(
            menu,
            |this: &Self| this.config.query_source,
            |this: &Self| this.config.search,
            &cx.entity(),
            |this, source, cx| this.pick_query_source(source, cx),
            |this, on, cx| {
                this.config.search = on;
                // The box keeps its text; the view shows the full catalog
                // while hidden.
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
        if self.resync_box {
            self.resync_box = false;
            self.sync_query_box(window, cx);
        }
        let max = self.max_index();
        let step = self.step_px();

        // All motion steps here and requests the next frame only while
        // something is still moving.
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
        // Publish once, when a user move settles on a new album.
        if !moving {
            let c = self.pos.round().max(0.) as usize;
            if self.publish_pending && self.centered != Some(c) {
                self.select_only(c, cx);
            }
            self.publish_pending = false;
        }

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
            // Bindings beat key listeners, so the workspace's left and right
            // would seek instead of reaching the listener below. PanelNav
            // takes the pair back while focused; TypeAhead adds space and tab
            // while a phrase is up.
            .key_context(panel::panel_nav_context(
                &self.type_ahead,
                self.type_ahead_at,
            ))
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
        let content: AnyElement = if self.cells.is_empty() {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .p(tokens::SPACE_MD)
                .text_center()
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
            // A row's floor shifts the covers up by half its strip so cover
            // and mirror center as one block.
            let refl_shift = if self.config.reflection && axis == Axis::Horizontal {
                (hero * REFL + REFL_GAP) / 2.0
            } else {
                0.0
            };
            let (cx_px, cy_px) = (w / 2.0, (h - LABEL_H) / 2.0 - refl_shift);
            let center = self.pos.round().max(0.) as usize;

            // Far covers first, so the nearer ones stack on top.
            let lo = (self.pos.floor() as i64 - self.visible()).max(0);
            let hi = (self.pos.ceil() as i64 + self.visible()).min(self.cells.len() as i64 - 1);
            let mut order: Vec<i64> = (lo..=hi).collect();
            order.sort_by(|a, b| {
                let da = (*b as f32 - self.pos).abs();
                let db = (*a as f32 - self.pos).abs();
                da.partial_cmp(&db).unwrap()
            });
            // A cover fading out drops from the pointer map before it stops
            // painting.
            self.hits = order
                .iter()
                .rev()
                .filter_map(|&ix| {
                    let d = ix as f32 - self.pos;
                    (self.edge_fade(d.abs()) >= HIT_OP)
                        .then(|| (ix as usize, self.outline(d, hero, cx_px, cy_px)))
                })
                .collect();

            // The id buys the hover state, the only signal the pointer left
            // without a move event.
            let mut shelf = div()
                .id("art-shelf")
                .relative()
                .flex_1()
                .min_h_0()
                .overflow_hidden();
            if self.config.glow {
                shelf = shelf.child(self.glow(hero, cx_px, cy_px));
            }
            // Every mirror paints under every cover. Interleaved, a near
            // mirror and a far cover would fight over the same pixels.
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
                // Any press might be a scrub, so the cover click waits for
                // the release.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.clear_type_ahead(cx);
                        this.flick.begin(event.position.along(axis));
                        this.coasting = true;
                        this.publish_pending = true;
                        this.touch_resume(cx);
                        cx.notify();
                    }),
                )
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
                            this.select_only(ix, cx);
                            this.navigate(ix, cx);
                        }
                    }),
                )
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
                // Banks sub-step travel so a trackpad's small deltas count.
                .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _, cx| {
                    this.touch_resume(cx);
                    // A notch arrives as 3 lines, so one notch moves one cover.
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
                // The canvas measures the frame and arms the live scrub's
                // window handlers in its paint hook.
                .child(
                    canvas(
                        {
                            let weak = cx.entity().downgrade();
                            move |bounds: Bounds<Pixels>, _, cx| {
                                if let Some(this) = weak.upgrade() {
                                    this.update(cx, |this, cx| {
                                        // No notify on a move alone; the
                                        // layout didn't change.
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
                // Keyed off the hovered cover, since the builder gets no
                // position.
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
                        // File actions take only local tracks, so a server's
                        // album offers none of them.
                        let state = this.read(cx).state.clone();
                        let copy_ids = ids.clone();
                        let files = state.library.read(cx).local_ids(&ids);
                        let has_files = !files.is_empty();
                        let reveal = files.first().copied();
                        let convert_state = state.clone();
                        let convert_ids = files.clone();
                        let menu = menu.when(has_files, |menu| {
                            menu.item(
                                PopupMenuItem::new(rox_i18n::t!("art-edit-tags"))
                                    .icon(Icon::default().path(icons::PENCIL))
                                    .on_click(move |_, _, cx| {
                                        rox_panel_api::openers::tags_editor(
                                            state.clone(),
                                            files.clone(),
                                            cx,
                                        );
                                    }),
                            )
                        });
                        // This menu doesn't go through `track_actions`, so the
                        // convert row is added here too. Gated on ffmpeg.
                        let menu = if has_files && rox_panel_api::openers::convert_available() {
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
                        let menu = panel::copy_ids_submenu(
                            menu,
                            this.read(cx).state.clone(),
                            copy_ids,
                            window,
                            cx,
                        );
                        let menu = panel::reveal_item(menu, this.read(cx).state.clone(), reveal);
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
