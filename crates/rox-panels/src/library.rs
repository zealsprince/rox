//! The dockable library panel that browses the shared catalog entity (which
//! is defined in `crate::catalog`). The catalog owns the app's library
//! database and only ever hands out the in-memory projection, per the
//! library service boundary. Panels are views over the shared catalog with
//! their own search config, so a duplicated panel filters independently. Double
//! clicking a track queues it straight on the shared player; single clicks
//! select, and the selection publishes app-wide for panels that display it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{
    div, prelude::*, px, rems, AnyElement, App, ClickEvent, Context, Div, Entity, EventEmitter,
    FocusHandle, Focusable, KeyDownEvent, ModifiersChangedEvent, MouseButton, ScrollStrategy,
    ScrollWheelEvent, SharedString, Stateful, Subscription, WeakEntity, Window, WindowHandle,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::table::{Column, ColumnSort, Table, TableDelegate, TableEvent, TableState};
use gpui_component::{Icon, IconName, Root, Side, Sizable, Size};
use rox_dock::{Panel, PanelEvent, PanelInfo, PanelState, TabPanel};
use rox_panel_api::actions::{TypeAheadNext, TypeAheadPrev};

use rox_core::fmt::{fmt_ago, fmt_ms, fmt_num};
use rox_core::QUEUE_CAP;
use rox_library::cue::TrackKey;
use rox_library::projection::{Projection, QueryField, QUERY_FIELDS};
use rox_library::view::{self, Group, Grouping, Row, ViewSpec};
use rox_services::backdrop::WindowBackdrop;

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::continuation;
use crate::design::{palette, tokens};
use crate::group_head::{
    self, effective_head_lines, ArtSide, HeadPiece, Headers, TileFace, MOSAIC,
};
use crate::panel::{self, AppState, PanelChrome, ResumeIdle, ScrubState};
use crate::panel_settings;
use crate::query::search::{SearchBox, SearchEvent};
use crate::query::shared_query::{QueryFilter, QuerySource, SharedQueryEvent};
use crate::selection::SelectionEvent;
use crate::settings::ui as settings_ui;
use crate::settings::GainModeSetting;
use crate::thumbs::Thumb;
use crate::track_ui::track_cells;
use crate::track_ui::track_drag::{PlayDrag, PlayDragPreview};

/// The header tiles' rounding knob ceiling, the panel frame sliders'
/// scale.
const ART_ROUNDING_MAX: f32 = 24.;

/// How far page up and page down step the keyboard cursor.
const PAGE_ROWS: isize = 25;

/// How long a keystroke-driven view rebuild waits for the next keystroke
/// before it runs. Long enough that typing a word starts one pass instead
/// of one per letter, short enough that a pause between words shows
/// results before the hand moves again.
const VIEW_DEBOUNCE: Duration = Duration::from_millis(100);

mod columns;

pub use columns::LibraryConfig;
use columns::*;

/// A group's codec, stream shape, and bitrate stat, resolving the interned
/// codec symbol before handing off to the shared [`group_head::quality`].
/// A disagreeing depth or rate goes over as 0, which `quality` already
/// drops: the same reading a group that agrees on nothing gets, since
/// neither has a shape to name.
fn group_quality(group: &Group, projection: &Projection) -> String {
    group_head::quality(
        group.codec_name(projection),
        group.min_kbps,
        group.max_kbps,
        group.bit_depth.unwrap_or(0),
        group.sample_rate_hz.unwrap_or(0),
    )
}

/// Everything one view pass reads, owned rather than borrowed so the pass
/// can run on the background executor. The projection and the canonical
/// order ride their `Arc`s: the catalog swaps them whole and never patches
/// one in place, so a pass in flight keeps working over the library it
/// started on and its result is thrown away by the generation check when a
/// newer one has landed since.
struct ViewInputs {
    projection: Arc<Projection>,
    order: Arc<Vec<u32>>,
    query: String,
    filter: rox_library::projection::FilterSet,
    similar: Option<(Arc<HashMap<i64, f32>>, bool)>,
    sort: Option<(rox_library::projection::SortKey, bool)>,
    group_by: GroupBy,
    /// How many header rows open each run, None while headers are off.
    head_rows: Option<u8>,
}

/// The view pass itself: the same call the panel used to make inline, with
/// nothing left in it that touches a window or an entity.
fn compute_rows(inputs: &ViewInputs) -> (Arc<Vec<Row>>, Vec<Group>) {
    let group_by = inputs.group_by;
    let key = move |projection: &Projection, row: u32| -> u64 {
        let i = row as usize;
        match group_by {
            GroupBy::Album => {
                (projection.album_artist[i] as u64) << 32 | projection.album[i] as u64
            }
            GroupBy::Artist => projection.album_artist[i] as u64,
            GroupBy::Genre => projection.genre[i] as u64,
            GroupBy::Year => projection.year[i] as u64,
        }
    };
    let grouping = inputs.head_rows.map(|head_rows| Grouping {
        head_rows,
        pre_sort: group_by.sort(),
        key: &key,
        discs: group_by == GroupBy::Album,
    });
    view::view_for(
        &inputs.projection,
        inputs.order.clone(),
        &ViewSpec {
            query: &inputs.query,
            filter: &inputs.filter,
            similar: inputs.similar.as_ref().map(|(map, desc)| (&**map, *desc)),
            sort: inputs.sort,
            grouping,
        },
    )
}

/// The slice of a view a click plays through: up to `cap` track rows
/// around `ix`, at most half of them behind it so Prev has somewhere to
/// step back to, and the rest ahead. A click near the end of the view
/// takes the shortfall out of the rows behind instead, so the window is
/// always full while the view has the rows to fill it.
///
/// Walks out from the click rather than listing the view's tracks first:
/// the old pass built a `Vec<usize>` of every track row before slicing
/// `cap` of them out of it, which is tens of megabytes and a full scan for
/// a double click on a big library. Header and disc rows are skipped, not
/// counted. None when `ix` isn't a track row.
///
/// Hands back the view indices of the window, in view order, and the
/// clicked row's offset inside it.
fn play_window(view: &[Row], ix: usize, cap: usize) -> Option<(Vec<usize>, usize)> {
    if cap == 0 || !matches!(view.get(ix)?, Row::Track(_)) {
        return None;
    }
    let track_rows =
        |range: std::ops::Range<usize>| range.filter(|&i| matches!(view[i], Row::Track(_)));
    // Behind first, since its share is the fixed one; the rows ahead take
    // whatever the budget has left.
    let mut behind: Vec<usize> = track_rows(0..ix).rev().take(cap / 2).collect();
    let ahead: Vec<usize> = track_rows(ix + 1..view.len())
        .take(cap - behind.len() - 1)
        .collect();
    // The view ran out ahead of the click, so the window slides back over
    // the rows it does have, the way the old slice did against the end.
    let short = cap - behind.len() - 1 - ahead.len();
    if short > 0 {
        let from = behind.last().copied().unwrap_or(ix);
        behind.extend(track_rows(0..from).rev().take(short));
    }
    let start = behind.len();
    behind.reverse();
    behind.push(ix);
    behind.extend(ahead);
    Some((behind, start))
}

/// Swap a finished pass into the table, unless a newer one was scheduled
/// while it ran. True when the rows landed, which is what the panel's
/// post-swap work (the restored scroll, the follow) hangs off.
fn install_view(
    table: &mut TableState<TrackTable>,
    generation: u64,
    view: Arc<Vec<Row>>,
    groups: Vec<Group>,
    cx: &mut Context<TableState<TrackTable>>,
) -> bool {
    if table.delegate().view_gen != generation {
        return false;
    }
    // Selection indices point into the old view; drop them along with the
    // widget's own focus row. The shared selection keeps the last explicit
    // pick, a view refresh is not one.
    let delegate = table.delegate_mut();
    delegate.view = view;
    delegate.groups = groups;
    delegate.selected.clear();
    delegate.sel_gen += 1;
    delegate.anchor = None;
    delegate.cursor = None;
    delegate.locate_playing(cx);
    table.clear_selection(cx);
    cx.notify();
    true
}

/// The table delegate: the column set and the rows one panel displays.
/// Held inside the panel's `TableState`; the panel swaps `view` when the
/// query or the catalog changes.
struct TrackTable {
    state: AppState,
    /// The owning panel, for dispatching context menu actions back to it.
    panel: WeakEntity<LibraryPanel>,
    /// Rows currently displayed: the canonical order or a column sort's,
    /// broken by group headers over whatever runs are adjacent, or flat
    /// search hits.
    view: Arc<Vec<Row>>,
    /// The current view's groups, what header rows index; empty when the
    /// view renders flat. Swapped together with `view`, always.
    groups: Vec<Group>,
    /// How the canonical order breaks into groups, and on what field.
    /// Copied from the panel like the heights: the view computation
    /// and the header render read them here, the knobs are stored on the
    /// panel.
    headers: Headers,
    group_by: GroupBy,
    /// The track rows' height at the stock font size, copied here
    /// because the header block math needs it beside the line height
    /// below, and the widget's size is held outside the delegate.
    row_height: f32,
    /// The extra height each row fills, same units; part of the row
    /// stride the block math spans, so it's kept beside the height.
    row_spacing: f32,
    /// One composed header line's height at the stock font size,
    /// independent of the rows: a block spans however many table rows its
    /// lines need.
    head_height: f32,
    /// The header lines' text size, same units, free of the line height;
    /// copied from the panel like the heights.
    head_text: f32,
    /// The header tiles' corner radius, copied from the panel like the
    /// heights: the tile renders here, the knob is stored on the panel.
    art_rounding: f32,
    /// Which side the header blocks' cover tile sits on, copied the
    /// same way.
    art_side: ArtSide,
    /// The tile's inset from the block edges, px at the stock font size;
    /// the tile shrinks to keep the square. Copied likewise.
    art_margin: f32,
    /// Open space carved off each header block's edges, same units; the
    /// canvas math reads them beside the heights.
    header_gap_above: f32,
    header_gap_below: f32,
    /// The header rows' cover tile knob, copied from the panel the
    /// same way.
    header_art: bool,
    /// Round the artist grouping's tiles to the full circle the artist
    /// wall uses, copied likewise; off keeps the rounding knob.
    portrait_circle: bool,
    /// What the genre grouping's tile shows, the genre grid's faces,
    /// copied likewise.
    genre_face: TileFace,
    /// Header rows on the list background instead of the Elevated tint,
    /// copied likewise.
    header_flush: bool,
    /// The composed lines the current mode's header blocks draw,
    /// copied from the panel's config. Never empty.
    head_lines: Vec<Vec<HeadPiece>>,
    /// The plays column's compact face, copied from the panel like the
    /// heights: the cell renders here, the knob is stored on the panel.
    compact_plays: bool,
    /// Selected rows as indices into `view`, track rows only, since headers
    /// take no selection. Cleared when the view swaps, since the indices
    /// point elsewhere afterwards.
    selected: HashSet<usize>,
    /// Where the next shift-click extends from: the last plain or
    /// toggle-clicked row.
    anchor: Option<usize>,
    /// The keyboard cursor: where arrows move from and enter plays from.
    /// Follows clicks, so keys and mouse hand off mid-browse.
    cursor: Option<usize>,
    columns: Vec<Column>,
    /// The headers the user renamed, keyed by column. Held here beside the
    /// columns so a language switch can tell a typed name from a resolved
    /// one, and so the layout dump can write them back out. An empty value
    /// is a header asked to draw blank, not a missing entry.
    labels: HashMap<String, String>,
    /// Sort on a plain click in the header rather than on the sort icon,
    /// copied from the panel. While it's on the widget's own sorting is
    /// switched off, so the click, the arrow, and the Alt-held column drag
    /// all run from here.
    sort_on_click: bool,
    /// The language the headers above were worded in. Their labels are
    /// resolved once and stored on the Column, so unlike the strings that
    /// resolve at render time they don't follow a language switch on
    /// their own, and the menu on each header re-reads the registry every
    /// frame. Without this the two disagree on screen.
    columns_locale: &'static str,
    /// The active sort: a column key and whether it descends. None is the
    /// canonical order. Stored on the delegate because the header click
    /// arrives here; the panel reads it back for the layout dump.
    sort: Option<(SharedString, bool)>,
    /// The playing track's id, resolved once per track change by the
    /// panel, and its row in the current view when the view holds it.
    playing_id: Option<i64>,
    playing_row: Option<usize>,
    /// The favourited track ids, what the heart column checks each row
    /// against. Refreshed off the library on a playlist change, so a toggle
    /// anywhere lights the same track here without a full view rebuild.
    favourites: HashSet<i64>,
    /// How much each track resembles the one playing, for the Similar
    /// column. Scored off the acoustic vectors on a background thread when
    /// the playing track changes, never in a paint: the pass over every
    /// vector is tens of milliseconds on a large library. Empty until the
    /// column is shown, and while nothing is playing.
    similar: Arc<HashMap<i64, f32>>,
    /// What `similar` holds the scores for: the track they were measured
    /// against and the acoustic model they were measured under, so a rescore
    /// runs when either moves and not otherwise. None while the map is empty,
    /// including after a scoring pass that found no vectors to rank, so the
    /// next look gets another go once something has described the library.
    similar_anchor: Option<(i64, String)>,
    /// Resolved file paths for the cover column, cached per track id on the
    /// cell's first paint so the thumbnail lookup does not re-query the
    /// catalog every frame. Paths are stable per id; cleared on reload.
    cover_paths: HashMap<i64, Option<PathBuf>>,
    /// Resolved file paths for the drag payload, cached per track id. A row's
    /// `on_drag` value is built eagerly every frame, so the id-to-path query
    /// caches here or a scrolled list would hit the catalog per row per frame.
    /// Same lifetime as `cover_paths`; cleared on reload.
    drag_keys: HashMap<i64, Option<TrackKey>>,
    /// Bumped on every selection change. Keys the drag-set cache below so it
    /// rebuilds only when the selection actually moves, not per frame. A view
    /// swap always clears the selection, so this catches those too.
    sel_gen: u64,
    /// Bumped every time a view pass is scheduled. The pass carries the
    /// number it was scheduled under and its result is dropped on arrival
    /// unless this still matches, so a slow pass over a big library can
    /// never overwrite the answer to a later keystroke.
    view_gen: u64,
    /// The wall clock the "added" column dates against, refreshed at most every
    /// half minute instead of a `SystemTime::now` per shown cell per frame;
    /// relative-time granularity is coarse enough that the small lag is unseen.
    added_now: i64,
    added_now_at: Instant,
    /// The multi-selection drag paths, in view order, built once per selection
    /// change and shared behind an Arc. A grab inside the selection hands every
    /// visible selected row this same Arc instead of rebuilding the whole set
    /// per row per frame.
    drag_set: Option<(u64, Arc<[TrackKey]>)>,
}

impl TrackTable {
    /// Take a header sort: mark the clicked column, remember what the
    /// list is sorted by, and schedule the pass. Called by the widget's
    /// own sort hook and, with click-to-sort on, by the header click that
    /// runs the cycle here instead.
    ///
    /// The view is scheduled rather than refreshed through the panel: the
    /// table entity is mid-update and the panel's refresh path would
    /// re-enter it. The panel reads the sort back for persistence via
    /// `dump`. Sorting ten million rows is a quarter of a second on
    /// integer ranks and near a second by title, so the pass goes to the
    /// background executor like every other one and the old rows stay up
    /// until it lands.
    fn apply_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        columns::mirror_sort(&mut self.columns, col_ix, sort);
        self.sort = match sort {
            ColumnSort::Ascending => Some((self.columns[col_ix].key.clone(), false)),
            ColumnSort::Descending => Some((self.columns[col_ix].key.clone(), true)),
            ColumnSort::Default => None,
        };
        let (query, filter) = self
            .panel
            .upgrade()
            .map(|panel| {
                let panel = panel.read(cx);
                (panel.effective_query(cx), panel.effective_filter(cx))
            })
            .unwrap_or_default();
        self.view_gen += 1;
        let generation = self.view_gen;
        let Some(inputs) = self.view_inputs(&query, &filter, cx) else {
            return;
        };
        let panel = self.panel.clone();
        cx.spawn(async move |table, cx| {
            let (view, groups) = cx
                .background_executor()
                .spawn(async move { compute_rows(&inputs) })
                .await;
            let installed = table
                .update(cx, |table, cx| {
                    install_view(table, generation, view, groups, cx)
                })
                .unwrap_or(false);
            // A sort is a landing like any other, so the panel's post-swap
            // work runs off it too. Without this the restored scroll and a
            // pending follow sit there until some unrelated refresh lands
            // and yanks the list out from under whoever was reading it.
            // The table's own update has finished by here, so the panel
            // can read it back without re-entering it.
            if installed {
                panel
                    .update(cx, |panel, cx| panel.on_view_installed(cx))
                    .ok();
            }
        })
        .detach();
        // The header's own arrow moved with the click, so repaint now
        // rather than waiting for the rows.
        cx.notify();
    }

    /// Reword the headers when the language has changed under them.
    ///
    /// Only the wording is touched. Order, widths, and the active sort
    /// are the user's arrangement and mean the same thing in every
    /// language, so rebuilding the columns outright would throw away a
    /// layout to fix a label. A language switch arrives as nothing but a
    /// repaint, so the header hears about it on its next one.
    fn reword_columns(&mut self) {
        let locale = rox_i18n::locale();
        if self.columns_locale == locale {
            return;
        }
        self.columns_locale = locale;
        columns::reword(&mut self.columns, &self.labels);
    }

    /// The current unix time the "added" column dates against, refreshed at
    /// most twice a minute so a wall of shown cells shares one read instead of
    /// each calling `SystemTime::now`.
    fn added_now(&mut self) -> i64 {
        if self.added_now_at.elapsed() >= Duration::from_secs(30) {
            self.added_now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(self.added_now);
            self.added_now_at = Instant::now();
        }
        self.added_now
    }

    /// The track a view row holds; None for a header row.
    fn track_at(&self, ix: usize) -> Option<u32> {
        match self.view.get(ix) {
            Some(&Row::Track(row)) => Some(row),
            _ => None,
        }
    }

    /// The drag payload for a grab on row `ix`. A grab inside a multi
    /// selection takes the whole set in view order; outside it, just that
    /// row, queue.rs's rule. Resolves through the same `keys_for` the play
    /// actions use, so a drop enqueues exactly what those queue. The value is
    /// built eagerly every frame, so keys come from `drag_keys`, filled per
    /// id on the first grab that needs it rather than a query per row per frame.
    fn drag_payload(&mut self, ix: usize, cx: &App) -> Option<PlayDrag> {
        let projection = self.state.library.read(cx).projection().cloned()?;
        let title = self
            .track_at(ix)
            .map(|row| projection.resolve(row).title.to_string())
            .unwrap_or_default();
        // A grab inside a multi-selection takes the whole set in view order,
        // built once per selection change and shared behind an Arc so it costs
        // a refcount bump per row, not a rebuild. Outside it, just this row.
        let keys: Arc<[TrackKey]> = if self.selected.len() > 1 && self.selected.contains(&ix) {
            if self.drag_set.as_ref().map(|(gen, _)| *gen) != Some(self.sel_gen) {
                let mut rows: Vec<usize> = self.selected.iter().copied().collect();
                rows.sort_unstable();
                let set: Arc<[TrackKey]> = self.resolve_drag_keys(&rows, &projection, cx).into();
                self.drag_set = Some((self.sel_gen, set));
            }
            self.drag_set.as_ref().map(|(_, set)| set.clone())?
        } else {
            self.resolve_drag_keys(&[ix], &projection, cx).into()
        };
        if keys.is_empty() {
            return None;
        }
        Some(PlayDrag {
            keys,
            title: title.into(),
        })
    }

    /// Resolve view rows to their tracks in row order, through a per-id cache
    /// so a drag never re-queries the catalog once a track is known.
    fn resolve_drag_keys(
        &mut self,
        rows: &[usize],
        projection: &Projection,
        cx: &App,
    ) -> Vec<TrackKey> {
        let ids: Vec<i64> = rows
            .iter()
            .filter_map(|&i| self.track_at(i))
            .map(|row| projection.db_id[row as usize])
            .collect();
        let mut keys = Vec::with_capacity(ids.len());
        for id in ids {
            let key = match self.drag_keys.get(&id) {
                Some(key) => key.clone(),
                None => {
                    let key = self
                        .state
                        .library
                        .read(cx)
                        .keys_for(&[id])
                        .ok()
                        .and_then(|mut keys| keys.pop());
                    self.drag_keys.insert(id, key.clone());
                    key
                }
            };
            if let Some(key) = key {
                keys.push(key);
            }
        }
        keys
    }

    /// The nearest track row from `ix` heading `forward`, bouncing off the
    /// ends; None only when the view holds no tracks. Cursor moves route
    /// through this, so the cursor never stops on a header.
    fn snap_to_track(&self, ix: usize, forward: bool) -> Option<usize> {
        let len = self.view.len();
        if len == 0 {
            return None;
        }
        let ix = ix.min(len - 1);
        let ahead = || (ix..len).find(|&i| self.track_at(i).is_some());
        let behind = || (0..=ix).rev().find(|&i| self.track_at(i).is_some());
        if forward {
            ahead().or_else(behind)
        } else {
            behind().or_else(ahead)
        }
    }

    /// The track rows under the group header line at `ix`, in view
    /// order; None when the row is no header. Every line of a block
    /// counts as its header, disc dividers don't open a group of their
    /// own.
    fn group_track_rows(&self, ix: usize) -> Option<Vec<usize>> {
        let g = match self.view.get(ix) {
            Some(&Row::Head(g, _)) => g,
            _ => return None,
        };
        let rows = (ix + 1..self.view.len())
            .take_while(|&i| !matches!(self.view.get(i), Some(&Row::Head(h, _)) if h != g))
            .filter(|&i| self.track_at(i).is_some())
            .collect();
        Some(rows)
    }

    /// The rendered height of one composed header line, scaled like the
    /// table scales its rows.
    fn line_px(&self) -> gpui::Pixels {
        px(self.head_height) * palette::row_scale()
    }

    /// The rendered block gaps and tile margin, same scaling again, so the
    /// insets hold their share of the block at any font size.
    fn gap_above_px(&self) -> gpui::Pixels {
        px(self.header_gap_above) * palette::row_scale()
    }

    fn gap_below_px(&self) -> gpui::Pixels {
        px(self.header_gap_below) * palette::row_scale()
    }

    fn art_margin_px(&self) -> gpui::Pixels {
        px(self.art_margin) * palette::row_scale()
    }

    /// The track rows' text size as a rem factor: the stock height keeps
    /// the stock 1 rem, and the text follows the height slider from
    /// there, floored so a dense list stays legible.
    fn row_font_scale(&self) -> f32 {
        (self.row_height / ROW_HEIGHT_STOCK).clamp(0.8, 1.8)
    }

    /// The header lines' factor: the text-size knob over its stock 1 rem,
    /// free of the line height, so the art (which spans the lines) grows
    /// without dragging the text along.
    fn head_font_scale(&self) -> f32 {
        self.head_text / HEAD_TEXT_STOCK
    }

    /// How many uniform table rows a header block spans: enough to hold
    /// its composed lines at their own height, so the line height moves
    /// free of the rows'. The scales cancel, so the stock-size values
    /// give the ratio.
    fn head_rows(&self) -> u8 {
        // One row per composed line: the table lays rows out at the
        // heights this delegate hands it, so a block's height is exactly
        // its lines plus the gaps, and nothing rounds to whole rows.
        self.head_lines.len().clamp(1, u8::MAX as usize) as u8
    }

    /// The edge length of an expanded header's cover tile: the composed
    /// lines' full height less the tile's own margin, so the art squares
    /// off against the text and scales smoothly with the line height.
    /// Scaled like the table scales its rows, so the square holds at any
    /// app font size or panel override.
    fn tile_side(&self) -> gpui::Pixels {
        let side = self.line_px() * self.head_lines.len() as f32 - self.art_margin_px() * 2.;
        if side < px(0.) {
            px(0.)
        } else {
            side
        }
    }

    /// Whether the tiles use the artist wall's full circle: grouped by
    /// artist with the circle knob on, the wall's default face.
    fn circled(&self) -> bool {
        self.group_by == GroupBy::Artist && self.portrait_circle
    }

    /// The block tile's corner radius: the rounding knob, or half the
    /// tile when the artist grouping uses the wall's circle.
    fn tile_rounding(&self) -> f32 {
        if self.circled() {
            f32::from(self.tile_side()) / 2.
        } else {
            self.art_rounding
        }
    }

    /// The heading look knobs packaged for the shared surface, read off
    /// the delegate the same way the tile side is. The year and
    /// details switches stay on: the composed lines already hold those
    /// choices. The circle applies to the inline art piece too, at that
    /// square's own radius.
    fn head_look(&self) -> group_head::HeadLook {
        group_head::HeadLook {
            tile_side: self.tile_side(),
            show_art: self.header_art,
            show_year: true,
            show_details: true,
            line_px: self.line_px(),
            art_side: self.art_side,
            art_margin: self.art_margin_px(),
            art_rounding: if self.circled() {
                f32::from(self.line_px() - tokens::SPACE_XS * 2.) / 2.
            } else {
                self.art_rounding
            },
            font_scale: self.head_font_scale(),
        }
    }

    /// An expanded header's cover tile, painted whole by each of the
    /// block's rows at `lift` (how far above this row the block's lines
    /// begin; negative drops it past the first row's gap), the last draw
    /// winning. Same image handles every time, so gpui decodes them
    /// once. Pending and missing use the same quiet placeholder, so an
    /// arriving cover fills the tile without shifting the text beside it.
    /// Grouped by genre the tile shows the configured genre face, the
    /// grid's looks: the cover mosaic plain or under the genre's wash,
    /// or a color card under its geometry.
    fn group_tile(
        &mut self,
        g: u32,
        lift: gpui::Pixels,
        cx: &mut Context<TableState<Self>>,
    ) -> AnyElement {
        if self.group_by == GroupBy::Genre {
            // The card faces paint no covers, so they skip the path
            // resolve and the thumbnail cache, the grid's economy.
            let paths = if self.genre_face.is_card() {
                Vec::new()
            } else {
                self.group_art_paths(g, cx)
            };
            let thumbs: Vec<Thumb> = paths
                .iter()
                .map(|path| {
                    self.state
                        .thumbs
                        .update(cx, |thumbs, cx| thumbs.get(path, cx))
                })
                .collect();
            let name = {
                let library = self.state.library.read(cx);
                self.groups
                    .get(g as usize)
                    .zip(library.projection())
                    .map(|(group, projection)| projection.resolve(group.first).genre.to_string())
                    .unwrap_or_default()
            };
            return group_head::genre_tile(
                self.genre_face,
                &thumbs,
                &name,
                self.tile_side(),
                self.art_rounding,
                lift,
                self.art_side,
                self.art_margin_px(),
            );
        }
        let thumb = self.group_thumb(g, cx);
        group_head::tile(
            thumb,
            self.tile_side(),
            self.tile_rounding(),
            lift,
            self.art_side,
            self.art_margin_px(),
        )
    }

    /// A group's single cover thumbnail, off its first resolved path; the
    /// inline art piece draws this too, since a line-tall square has no
    /// room for the genre mosaic. Grouped by artist the portrait service
    /// is tried first, the artist wall's face, with the lead record's
    /// cover standing in while a lookup runs or after a settled miss.
    fn group_thumb(&mut self, g: u32, cx: &mut Context<TableState<Self>>) -> Thumb {
        if let Some(portrait) = self.group_portrait(g, cx) {
            return portrait;
        }
        let paths = self.group_art_paths(g, cx);
        match paths.first() {
            Some(path) => self
                .state
                .thumbs
                .update(cx, |thumbs, cx| thumbs.get(path, cx)),
            None => Thumb::Missing,
        }
    }

    /// The artist grouping's portrait through the shared service the
    /// artist wall draws from, under the same name the header line shows.
    /// None for every other grouping, while a lookup is in flight, and
    /// for a name the services have nothing under; those fall back to
    /// the cover. An arriving face notifies the service, and the panel's
    /// subscription repaints this into the tile.
    fn group_portrait(&mut self, g: u32, cx: &mut Context<TableState<Self>>) -> Option<Thumb> {
        if self.group_by != GroupBy::Artist {
            return None;
        }
        let name = {
            let library = self.state.library.read(cx);
            let projection = library.projection()?;
            let v = projection.resolve(self.groups.get(g as usize)?.first);
            if v.album_artist.is_empty() {
                v.artist.to_string()
            } else {
                v.album_artist.to_string()
            }
        };
        self.state
            .portraits
            .update(cx, |portraits, cx| portraits.get(&name, cx))
            .map(Thumb::Ready)
    }

    /// The resolved cover paths a group's art loads by, cached on the
    /// group: the run's first track for album and artist grouping, the
    /// first [`MOSAIC`] distinct tagged albums for genre's mosaic. Empty
    /// for the unknown bucket (an empty grouped field), which keeps the
    /// placeholder instead of whichever loose track's art comes back first.
    fn group_art_paths(&mut self, g: u32, cx: &mut Context<TableState<Self>>) -> Vec<PathBuf> {
        if let Some(paths) = self
            .groups
            .get(g as usize)
            .and_then(|group| group.art.clone())
        {
            return paths;
        }
        let paths = {
            let library = self.state.library.read(cx);
            let ids: Vec<i64> = self
                .groups
                .get(g as usize)
                .zip(library.projection())
                .map(|(group, projection)| {
                    let v = projection.resolve(group.first);
                    match self.group_by {
                        GroupBy::Album if !v.album.is_empty() => {
                            vec![projection.db_id[group.first as usize]]
                        }
                        GroupBy::Artist if !v.album_artist.is_empty() => {
                            vec![projection.db_id[group.first as usize]]
                        }
                        GroupBy::Genre if !v.genre.is_empty() => self.mosaic_ids(g, projection),
                        _ => Vec::new(),
                    }
                })
                .unwrap_or_default();
            library.paths_for(&ids).unwrap_or_default()
        };
        if let Some(group) = self.groups.get_mut(g as usize) {
            group.art = Some(paths.clone());
        }
        paths
    }

    /// The genre mosaic's track ids, the genre grid's pick: the first
    /// track of each of the run's first [`MOSAIC`] distinct tagged
    /// albums, walked off the view's own rows under the group's header.
    fn mosaic_ids(&self, g: u32, projection: &Projection) -> Vec<i64> {
        let Some(start) = self
            .view
            .iter()
            .position(|row| matches!(row, Row::Head(h, 0) if *h == g))
        else {
            return Vec::new();
        };
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        for row in &self.view[start..] {
            match *row {
                Row::Head(h, _) if h != g => break,
                Row::Track(r) => {
                    if projection.resolve(r).album.is_empty() {
                        continue;
                    }
                    if seen.insert(projection.album[r as usize]) {
                        ids.push(projection.db_id[r as usize]);
                        if ids.len() == MOSAIC {
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        ids
    }

    /// One table row of a group's header block. The group resolves once
    /// into the full [`group_head::GroupHead`], then the row draws the
    /// whole block canvas (the cover tile and every composed line at the
    /// line height) shifted up past the block rows above it. Every row
    /// of the block paints the same canvas whole, unclipped, so the last
    /// one's paint is the one that shows: one seamless draw whatever the
    /// line and row heights are. Grouped by album every field fills; the other
    /// groupings resolve the name they key on plus the count and time, so
    /// album pieces just drop out of their lines. The tile stays for
    /// artist runs (the artist's portrait, or their lead cover while it
    /// loads) and genre runs (the genre grid's cover mosaic); the year
    /// grouping has no one image to show, so it alone goes bare.
    fn render_head_row(
        &mut self,
        row_ix: usize,
        g: u32,
        line: u8,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        let expanded = self.headers == Headers::Expanded;
        let by_album = self.group_by == GroupBy::Album;
        let with_art = self.group_by != GroupBy::Year;
        let has_tile = expanded && with_art && self.header_art;
        // One row per composed line, sized to it by the delegate's
        // `row_height`, the gaps taken out of the block's first and last rows:
        // the block's height is exactly its content, so the gap and line
        // height knobs read pixel for pixel with nothing left to round.
        let lines = self.head_lines.len().max(1);
        let first = line == 0;
        let last = line as usize + 1 >= lines;
        let line_px = self.line_px();
        let gap_above = if first { self.gap_above_px() } else { px(0.) };
        let gap_below = if last { self.gap_below_px() } else { px(0.) };
        // The tile spans the block's lines; every row paints it whole at
        // its own offset, the last draw winning, so it stays one
        // seamless square. The first row drops it past its gap, the
        // rest lift it back past the lines already painted.
        let lift = if first {
            -self.gap_above_px()
        } else {
            line_px * line as f32
        };
        let tile = has_tile.then(|| self.group_tile(g, lift, cx));
        // The inline art piece draws the single cover (or portrait); a
        // line-tall square has no room for the genre mosaic.
        let inline_art = with_art && self.head_lines.iter().any(|l| l.contains(&HeadPiece::Art));
        let mut head = match (
            self.groups.get(g as usize),
            self.state.library.read(cx).projection(),
        ) {
            (Some(group), Some(projection)) => {
                let v = projection.resolve(group.first);
                let name = match self.group_by {
                    GroupBy::Album | GroupBy::Artist => {
                        // Rows migrated from before the album artist
                        // column have an empty one until a rescan
                        // re-reads their tags; the first track's artist
                        // stands in rather than "unknown".
                        if v.album_artist.is_empty() {
                            v.artist.to_string()
                        } else {
                            v.album_artist.to_string()
                        }
                    }
                    GroupBy::Genre => v.genre.to_string(),
                    GroupBy::Year => {
                        if v.year == 0 {
                            String::new()
                        } else {
                            v.year.to_string()
                        }
                    }
                };
                // The heading's own readings, from the same fields the
                // rows below read. The name line follows whichever of the
                // two artist fields stood in for it above; year and genre
                // headings have no sort name of their own.
                let name_reading = match self.group_by {
                    GroupBy::Album | GroupBy::Artist => {
                        if v.album_artist.is_empty() {
                            v.artist_sort
                        } else {
                            v.album_artist_sort
                        }
                    }
                    GroupBy::Genre | GroupBy::Year => "",
                };
                group_head::GroupHead {
                    name: SharedString::from(name),
                    name_reading: SharedString::from(name_reading.to_string()),
                    album: if by_album {
                        SharedString::from(v.album.to_string())
                    } else {
                        SharedString::default()
                    },
                    album_reading: if by_album {
                        SharedString::from(v.album_sort.to_string())
                    } else {
                        SharedString::default()
                    },
                    year: if by_album { v.year } else { 0 },
                    genre: if by_album {
                        SharedString::from(v.genre.to_string())
                    } else {
                        SharedString::default()
                    },
                    quality: if by_album {
                        SharedString::from(group_quality(group, projection))
                    } else {
                        SharedString::default()
                    },
                    tracks: group.tracks,
                    total_ms: group.total_ms,
                    tiled: with_art,
                    thumb: None,
                }
            }
            _ => group_head::GroupHead {
                tiled: with_art,
                ..Default::default()
            },
        };
        if inline_art {
            head.thumb = Some(self.group_thumb(g, cx));
        }
        let look = self.head_look();
        let bg = if self.header_flush {
            palette::bg_root()
        } else {
            palette::bg_header()
        };
        // The panel body already painted the list surface under every row,
        // so a header color that resolves to that same surface has nothing
        // to add: painting it anyway lays a second coat, which stops
        // matching the moment surfaces go translucent. A header meant to
        // sit on the list's own color matches by not painting at all.
        let tinted = bg != palette::bg_root();
        // The row is its line: the strip renders in place at the line
        // height, past its gap share, and the tint hugs it so the gaps
        // show the list. A row with no gap share tints its own
        // background, which keeps the block's bottom hairline drawing
        // over it; the edge rows draw the tint as a child slice.
        let strip = self
            .head_lines
            .get(line as usize)
            .map(|pieces| group_head::line_content(pieces, &head, &look, expanded));
        div()
            .id(("row", row_ix))
            // A click selects the album, a double click plays it, so the
            // strip uses the same pointer a track row does.
            .cursor_pointer()
            // The block reads as one: no border between its rows. The
            // width stays, so rows keep their height.
            .when(!last, |d| d.border_color(gpui::transparent_black()))
            .map(|d| {
                if !tinted {
                    d
                } else if gap_above <= px(0.) && gap_below <= px(0.) {
                    d.bg(bg)
                } else {
                    d.child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .top(gap_above)
                            .h(line_px)
                            .bg(bg),
                    )
                }
            })
            .when_some(tile, |d, tile| d.child(tile))
            .when_some(strip, |d, strip| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(gap_above)
                        .h(line_px)
                        .child(strip),
                )
            })
    }

    /// The slim strip opening one disc's run inside a multi-disc group,
    /// a full-width line like the header rows so it stays put when wide
    /// column sets scroll sideways.
    fn render_disc_row(&mut self, row_ix: usize, disc: u16) -> Stateful<Div> {
        div().id(("row", row_ix)).child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .flex_row()
                .items_center()
                .px(tokens::SPACE_SM)
                .text_color(palette::text_muted())
                .child(rox_i18n::t!("library-disc", number = disc as u64)),
        )
    }

    /// Split a leading `field:` pin off a type-ahead phrase, the same
    /// vocabulary the shared query's `field:"value"` terms use ([`QUERY_FIELDS`]),
    /// so typing `artist:` narrows the jump to that column. Unrecognized
    /// or non-textual prefixes (an unknown name, or a numeric-only field
    /// like `rating:`) fall through and the whole phrase reads as one
    /// literal, same as an unknown prefix in the query box.
    fn type_ahead_pin(phrase: &str) -> Option<(&'static str, &str)> {
        let (name, rest) = phrase.split_once(':')?;
        let (_, field) = QUERY_FIELDS
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))?;
        let key = match field {
            QueryField::Title => "title",
            QueryField::Artist => "artist",
            QueryField::AlbumArtist => "album_artist",
            QueryField::Album => "album",
            QueryField::Genre => "genre",
            QueryField::Codec => "codec",
            QueryField::Year
            | QueryField::Folder
            | QueryField::Rating
            | QueryField::Plays
            | QueryField::Added => return None,
        };
        Some((key, rest))
    }

    /// The next row the typed phrase jumps to, from the cursor on,
    /// wrapping. A plain phrase matches the start of any word in any of
    /// the row's naming fields: title, artist, album artist, album. A
    /// `field:` pin narrows it to one column, which is also how the
    /// repeat-heavy fields (genre, codec) are reached: in the open sweep
    /// they'd sit on nearly every row and bury the real hits. ASCII
    /// case-insensitive, like search.
    fn find_prefix(&self, prefix: &str, include_current: bool, cx: &App) -> Option<usize> {
        let len = self.view.len();
        if len == 0 {
            return None;
        }
        let start = match self.cursor {
            Some(cursor) if include_current => cursor,
            Some(cursor) => cursor + 1,
            None => 0,
        };
        self.find_in((0..len).map(move |i| (start + i) % len), prefix, cx)
    }

    /// The neighbouring match in either direction, for Tab and Shift+Tab
    /// cycling a live phrase's hits.
    fn find_step(&self, prefix: &str, back: bool, cx: &App) -> Option<usize> {
        self.find_in(
            panel::type_ahead_scan(self.view.len(), self.cursor, back),
            prefix,
            cx,
        )
    }

    /// The first row along `order` the phrase matches, [`find_prefix`]'s
    /// rules.
    fn find_in(&self, order: impl Iterator<Item = usize>, prefix: &str, cx: &App) -> Option<usize> {
        let library = self.state.library.read(cx);
        let projection = library.projection()?;
        let pin = Self::type_ahead_pin(prefix);
        order.into_iter().find(|&ix| {
            let Some(row) = self.track_at(ix) else {
                return false;
            };
            let v = projection.resolve(row);
            match pin {
                Some((field, needle)) => {
                    let text = match field {
                        "title" => v.title,
                        "artist" => v.artist,
                        "album_artist" => v.album_artist,
                        "album" => v.album,
                        "genre" => v.genre,
                        _ => v.codec,
                    };
                    panel::type_ahead_hit(text, needle)
                }
                None => [v.title, v.artist, v.album_artist, v.album]
                    .iter()
                    .any(|text| panel::type_ahead_hit(text, prefix)),
            }
        })
    }

    /// Re-locate the playing track in the current view: one scan per view
    /// swap or track change, never per frame.
    fn locate_playing(&mut self, cx: &App) {
        let row = self.playing_id.and_then(|id| {
            let library = self.state.library.read(cx);
            let projection = library.projection()?;
            self.view
                .iter()
                .position(|&row| matches!(row, Row::Track(r) if projection.db_id[r as usize] == id))
        });
        self.playing_row = row;
    }

    /// Everything the view pass needs, read off the catalog entity and the
    /// delegate here so the pass itself can run on a background thread.
    /// None while the catalog has no projection yet, which is an empty
    /// view.
    fn view_inputs(
        &self,
        query: &str,
        filter: &rox_library::projection::FilterSet,
        cx: &App,
    ) -> Option<ViewInputs> {
        let library = self.state.library.read(cx);
        let projection = library.projection()?.clone();
        Some(ViewInputs {
            projection,
            order: library.order(),
            query: query.to_string(),
            filter: filter.clone(),
            // The similar column scores off the delegate's own map, so it
            // hands the scores over rather than naming a projection field.
            similar: self
                .sort
                .as_ref()
                .and_then(|(key, desc)| (key.as_ref() == "similar").then_some(*desc))
                .map(|desc| (self.similar.clone(), desc)),
            sort: self
                .sort
                .as_ref()
                .and_then(|(key, desc)| sort_key(key).map(|key| (key, *desc))),
            group_by: self.group_by,
            head_rows: (self.headers != Headers::Off).then(|| self.head_rows()),
        })
    }

    /// Append the owning panel's dropdown items to a row context menu.
    /// Called while the table entity is mid-update, so the panel's
    /// `dropdown_menu` must not read the table entity at build time (its
    /// click handlers may, they run after the update ends).
    fn panel_menu(&self, menu: PopupMenu, window: &mut Window, cx: &mut App) -> PopupMenu {
        let Some(panel) = self.panel.upgrade() else {
            return menu;
        };
        panel.update(cx, |panel, cx| panel.dropdown_menu(menu, window, cx))
    }

    /// Resolve the selected rows to db ids in view order and publish them
    /// on the shared selection.
    fn publish_selection(&self, cx: &mut App) {
        let Some(projection) = self.state.library.read(cx).projection().cloned() else {
            return;
        };
        let mut rows: Vec<usize> = self.selected.iter().copied().collect();
        rows.sort_unstable();
        let ids: Vec<i64> = rows
            .iter()
            .filter_map(|&ix| self.track_at(ix))
            .map(|row| projection.db_id[row as usize])
            .collect();
        // The delegate publishes on the panel's behalf, so the pick uses
        // the panel's id: that's what a scoped drawer and a
        // selection-following view match against.
        let source = self.panel.entity_id();
        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, source, cx));
    }
}

impl TableDelegate for TrackTable {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.view.len()
    }

    /// Track rows are the plain ones: they take the stripe and hover
    /// washes. A header block's rows compose one canvas (disc dividers sit
    /// inside a run the same way), so a per-row wash would band it.
    fn plain_row(&self, row_ix: usize) -> bool {
        self.track_at(row_ix).is_some()
    }

    /// A header line's row sizes to the line itself, the block's first
    /// and last rows taking the gaps; every other row takes the
    /// table's uniform stride. That frees the blocks from whole-row
    /// rounding: their height is exactly their content.
    fn row_height(&self, row_ix: usize) -> Option<gpui::Pixels> {
        match self.view.get(row_ix) {
            Some(&Row::Head(_, line)) => {
                let mut h = self.line_px();
                if line == 0 {
                    h += self.gap_above_px();
                }
                if line as usize + 1 >= self.head_lines.len().max(1) {
                    h += self.gap_below_px();
                }
                Some(h)
            }
            _ => None,
        }
    }

    /// A cheap fingerprint of everything `row_height` reads: the knobs,
    /// the render scale, the line count, and the view's identity. The
    /// table rebuilds its per-row size cache when this moves.
    fn row_heights_version(&self) -> u64 {
        let mut h: u64 = 0;
        for v in [
            self.head_height,
            self.header_gap_above,
            self.header_gap_below,
            palette::row_scale(),
        ] {
            h = h
                .wrapping_mul(0x100000001b3)
                .wrapping_add(v.to_bits() as u64);
        }
        h = h
            .wrapping_mul(0x100000001b3)
            .wrapping_add(self.head_lines.len() as u64);
        h ^ (Arc::as_ptr(&self.view) as u64)
    }

    fn column(&self, col_ix: usize, _: &App) -> &Column {
        &self.columns[col_ix]
    }

    /// The header cell: the stock label plus a right-click menu that
    /// renames this header and toggles the shown columns in place, the
    /// customize window's chips without the trip there. The table's own
    /// right-click menu stays a row affair; over the header it builds
    /// empty and never shows, so the two menus don't stack.
    ///
    /// With click-to-sort on, the cell also carries the sort: the widget's
    /// icon and its click are switched off (its `perform_sort` is private,
    /// so the cycle runs here instead), and the arrow for the sorted
    /// column draws beside the label. Alt+click is the column drag's grab,
    /// so it never sorts.
    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        self.reword_columns();
        let shown: HashSet<String> = self.columns.iter().map(|c| c.key.to_string()).collect();
        let panel = self.panel.clone();
        let key = self.columns[col_ix].key.to_string();
        let renamed = self.labels.contains_key(&key);
        let sorts = self.sort_on_click && columns::sortable(&key);
        // Our own arrow, drawn only for the column the list is sorted by,
        // at the size and in the shapes the widget's own icon uses.
        let arrow =
            sorts
                .then(|| self.columns[col_ix].sort)
                .flatten()
                .and_then(|sort| match sort {
                    ColumnSort::Ascending => Some(IconName::SortAscending),
                    ColumnSort::Descending => Some(IconName::SortDescending),
                    ColumnSort::Default => None,
                });
        div()
            .size_full()
            .id(("th", col_ix))
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(div().flex_1().child(self.column(col_ix, cx).name.clone()))
            .children(arrow.map(|icon| {
                Icon::new(icon)
                    .size_3()
                    .text_color(palette::text_muted())
                    .into_any_element()
            }))
            .when(sorts, |d| {
                d.cursor_pointer().on_click(cx.listener(
                    move |table, event: &ClickEvent, window, cx| {
                        // Alt is the grab that reorders the columns, so a
                        // press that ends without moving isn't a sort.
                        if event.modifiers().alt {
                            return;
                        }
                        let next = match table.delegate().columns.get(col_ix).and_then(|c| c.sort) {
                            Some(ColumnSort::Descending) => ColumnSort::Ascending,
                            Some(ColumnSort::Ascending) => ColumnSort::Default,
                            _ => ColumnSort::Descending,
                        };
                        table.delegate_mut().apply_sort(col_ix, next, window, cx);
                    },
                ))
            })
            .context_menu(move |mut menu, _, _| {
                let renaming = panel.clone();
                let key_for_rename = key.clone();
                menu = menu.item(
                    PopupMenuItem::new(rox_i18n::t!("library-column-rename"))
                        .icon(Icon::default().path(icons::PENCIL))
                        .on_click(move |_, _, cx| {
                            if let Some(panel) = renaming.upgrade() {
                                let key = key_for_rename.clone();
                                panel.update(cx, |panel, cx| panel.open_column_rename(key, cx));
                            }
                        }),
                );
                if renamed {
                    let resetting = panel.clone();
                    let key_for_reset = key.clone();
                    menu = menu.item(
                        PopupMenuItem::new(rox_i18n::t!("library-column-rename-reset"))
                            .icon(Icon::default().path(icons::REFRESH_CW))
                            .on_click(move |_, _, cx| {
                                if let Some(panel) = resetting.upgrade() {
                                    let key = key_for_reset.clone();
                                    panel.update(cx, |panel, cx| {
                                        panel.set_column_label(key, None, cx)
                                    });
                                }
                            }),
                    );
                }
                menu = menu.separator();
                for def in columns::offered() {
                    let key = def.key;
                    let panel = panel.clone();
                    menu = menu.item(
                        PopupMenuItem::new(def.label)
                            .checked(shown.contains(key))
                            .on_click(move |_, _, cx| {
                                if let Some(panel) = panel.upgrade() {
                                    panel.update(cx, |panel, cx| panel.toggle_column(key, cx));
                                }
                            }),
                    );
                }
                menu
            })
    }

    /// The header sort hook. The widget has already advanced the clicked
    /// column's cycle (canonical -> descending -> ascending) in its own
    /// column state, so all that's left is taking it, which is the same
    /// thing a click-to-sort header does for itself.
    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.apply_sort(col_ix, sort, window, cx);
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        // A group header line is one full-width strip over emptied cells,
        // since the table has no row-spanning cell. It hangs off the row
        // itself, outside the horizontally scrolled cell region, so the
        // title stays put when wide column sets scroll sideways.
        match self.view.get(row_ix).copied() {
            Some(Row::Head(g, line)) => return self.render_head_row(row_ix, g, line, cx),
            Some(Row::Disc(disc)) => return self.render_disc_row(row_ix, disc),
            _ => {}
        }
        // The same wash the widget theme paints its own focus row with, so
        // multi-selected rows read as one set. The playing row uses the
        // highlight role instead, a faint cut of it, so it stays apart
        // from the accent-washed selection.
        let selected = self.selected.contains(&row_ix);
        // The row is a drag source: dragging takes the grabbed row, or the
        // whole set when the grab starts inside a multi-selection, onto a drop
        // target that queues it. Resolved here so the payload is ready for
        // the frame.
        let drag = self.drag_payload(row_ix, cx);
        div()
            // Group bounds resolve innermost-first, so one shared name
            // still scopes each cell's group_hover to its own row: the
            // rating cell fades its unrated stars in on row hover.
            .group(track_cells::ROW_GROUP)
            .id(("row", row_ix))
            // The cells inherit this, so the text follows the row height
            // slider instead of floating small in a tall row.
            .text_size(rems(self.row_font_scale()))
            .cursor_pointer()
            .when(selected, |d| d.bg(palette::alpha(palette::accent(), 0x26)))
            .when(self.playing_row == Some(row_ix) && !selected, |d| {
                d.bg(palette::alpha(palette::highlight(), 0x12))
            })
            .when_some(drag, |d, drag| {
                d.on_drag(drag, |drag, _pos, _window, cx| {
                    cx.new(|_| PlayDragPreview {
                        title: drag.title.clone(),
                        extra: drag.len().saturating_sub(1),
                    })
                })
            })
    }

    /// The row context menu. A right click inside the selection acts on the
    /// whole set; outside it, the click reselects just that row first, so
    /// the menu always acts on what's highlighted. A group header stands
    /// for its album: the click selects the whole group, and the play item
    /// reads Play Album. The panel's own menu is appended after the track
    /// actions: the panel body hands its right-click to the table
    /// (`content_context_menu`), so this menu is the only one a click over
    /// the list opens, and it must not dead-end at Play. Disc dividers get
    /// the panel menu alone.
    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let album = self.group_track_rows(row_ix);
        if self.track_at(row_ix).is_none() && album.is_none() {
            return self.panel_menu(menu, window, cx);
        }
        if let Some(rows) = &album {
            self.selected = rows.iter().copied().collect();
            self.sel_gen += 1;
            self.anchor = rows.first().copied();
            self.cursor = rows.first().copied();
            self.publish_selection(cx);
            cx.notify();
        } else if !self.selected.contains(&row_ix) {
            self.selected = HashSet::from([row_ix]);
            self.sel_gen += 1;
            self.anchor = Some(row_ix);
            self.publish_selection(cx);
            cx.notify();
        }
        let mut rows: Vec<usize> = self.selected.iter().copied().collect();
        rows.sort_unstable();
        // The selection as db ids, resolved now so the editor gets this
        // set even if another panel publishes over the shared selection
        // before the click is handled.
        let ids: Vec<i64> = self
            .state
            .library
            .read(cx)
            .projection()
            .map(|projection| {
                rows.iter()
                    .filter_map(|&ix| self.track_at(ix))
                    .map(|row| projection.db_id[row as usize])
                    .collect()
            })
            .unwrap_or_default();
        let panel = self.panel.clone();
        let label = if album.is_some() {
            if self.group_by == GroupBy::Album {
                rox_i18n::t!("library-play-album").to_string()
            } else {
                rox_i18n::t!("library-play-group").to_string()
            }
        } else if rows.len() > 1 {
            rox_i18n::t!("library-play-tracks", count = rows.len() as u64).to_string()
        } else {
            rox_i18n::t!("library-play").to_string()
        };
        // A single row plays from it through the view, the double click's
        // move; a set or a group queues exactly the highlighted rows.
        let single_row = album.is_none() && rows.len() <= 1;
        let from_row = single_row.then_some(row_ix);
        let play_panel = panel.clone();
        let play_rows = rows.clone();
        let menu = panel::track_actions(
            menu,
            self.state.clone(),
            ids,
            label,
            window,
            cx,
            move |_, cx| {
                let Some(panel) = play_panel.upgrade() else {
                    return;
                };
                panel.update(cx, |panel, cx| match from_row {
                    Some(ix) => panel.play_from(ix, cx),
                    None => panel.play_rows(play_rows.clone(), cx),
                });
            },
        );
        // Filter the panel's search down to the clicked row's album or artist,
        // the cheap faceted browse. Only for a single clicked track row: a group
        // header stands for a whole album already, and a multi-row set has no
        // one album or artist to pin. An empty field skips its entry, nothing
        // to filter by.
        let menu = if album.is_none() && rows.len() == 1 {
            let (jump_album, jump_artist) = self
                .state
                .library
                .read(cx)
                .projection()
                .and_then(|projection| {
                    self.track_at(row_ix).map(|row| {
                        let v = projection.resolve(row);
                        (v.album.to_string(), v.artist.to_string())
                    })
                })
                .unwrap_or_default();
            let mut menu = menu;
            if !jump_album.is_empty() || !jump_artist.is_empty() {
                menu = menu.separator();
            }
            if !jump_album.is_empty() {
                let album_panel = panel.clone();
                menu = menu.item(
                    PopupMenuItem::new(rox_i18n::t!("library-filter-by-album"))
                        .icon(Icon::default().path(icons::DISC))
                        .on_click(move |_, _, cx| {
                            let Some(panel) = album_panel.upgrade() else {
                                return;
                            };
                            let album = jump_album.clone();
                            panel.update(cx, |panel, cx| panel.jump_to_query("album", &album, cx));
                        }),
                );
            }
            if !jump_artist.is_empty() {
                let artist_panel = panel.clone();
                menu = menu.item(
                    PopupMenuItem::new(rox_i18n::t!("library-filter-by-artist"))
                        .icon(Icon::default().path(icons::MIC))
                        .on_click(move |_, _, cx| {
                            let Some(panel) = artist_panel.upgrade() else {
                                return;
                            };
                            let artist = jump_artist.clone();
                            panel
                                .update(cx, |panel, cx| panel.jump_to_query("artist", &artist, cx));
                        }),
                );
            }
            // Play what sounds like the clicked track. Only offered once the
            // pass has actually described something: the switch alone permits
            // the vectors, it doesn't build them, and the action without them
            // is a menu entry that does nothing.
            if crate::settings::similarity_ready() {
                let similar_panel = panel.clone();
                menu = menu.item(
                    PopupMenuItem::new(rox_i18n::t!("library-play-similar"))
                        .icon(Icon::default().path(icons::AUDIO_WAVEFORM))
                        .on_click(move |_, _, cx| {
                            let Some(panel) = similar_panel.upgrade() else {
                                return;
                            };
                            panel.update(cx, |panel, cx| panel.play_similar(row_ix, cx));
                        }),
                );
            }
            menu
        } else {
            menu
        };
        self.panel_menu(menu.separator(), window, cx)
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        // Header rows draw in render_tr; their cells stay empty.
        let Some(row) = self.track_at(row_ix) else {
            return div().into_any_element();
        };
        let Some(projection) = self.state.library.read(cx).projection().cloned() else {
            return div().into_any_element();
        };
        let v = projection.resolve(row);
        let playing = self.playing_row == Some(row_ix);
        let readings = crate::settings::show_readings();
        let cell = div().truncate();
        // Copied out so the cover arm can borrow the delegate mutably (its
        // path cache) without the match still holding `self.columns`.
        let key = self.columns[col_ix].key.clone();
        if key.as_ref() == "cover" {
            let id = projection.db_id[row as usize];
            let path = match self.cover_paths.get(&id) {
                Some(path) => path.clone(),
                None => {
                    let path = self
                        .state
                        .library
                        .read(cx)
                        .paths_for(&[id])
                        .ok()
                        .and_then(|mut paths| paths.pop());
                    self.cover_paths.insert(id, path.clone());
                    path
                }
            };
            let thumb =
                crate::track_ui::track_columns::cover_thumb(&self.state, path.as_deref(), true, cx);
            // The delegate's own row height, not the shared stock one: the
            // cover square grows with the height knob like the rest of the
            // table's rows do.
            return crate::track_ui::track_columns::cover_cell(&thumb, self.row_height)
                .into_any_element();
        }
        let cell = match key.as_ref() {
            "track" => cell
                .text_color(palette::text_muted())
                .child(fmt_num(v.track_no)),
            // The four name columns carry the reading after the name when
            // the switch is on and the name is in a script this alphabet
            // can't sound out; the four sort columns below show the same
            // string on its own, which is why they don't go through the
            // helper.
            "title" => cell
                .when(playing, |d| d.text_color(palette::accent()))
                .child(panel::named(v.title, v.title_sort, readings)),
            "artist" => cell
                .text_color(palette::text_secondary())
                .child(panel::named(v.artist, v.artist_sort, readings)),
            "album_artist" => cell
                .text_color(palette::text_secondary())
                .child(panel::named(v.album_artist, v.album_artist_sort, readings)),
            "album" => cell
                .text_color(palette::text_secondary())
                .child(panel::named(v.album, v.album_sort, readings)),
            // The four sort tags. A row without one draws an empty cell
            // rather than falling back to the display name, since which
            // rows actually carry the tag is the whole reason to show the
            // column.
            "title_sort" => cell
                .text_color(palette::text_muted())
                .child(SharedString::from(v.title_sort.to_string())),
            "artist_sort" => cell
                .text_color(palette::text_muted())
                .child(SharedString::from(v.artist_sort.to_string())),
            "album_artist_sort" => cell
                .text_color(palette::text_muted())
                .child(SharedString::from(v.album_artist_sort.to_string())),
            "album_sort" => cell
                .text_color(palette::text_muted())
                .child(SharedString::from(v.album_sort.to_string())),
            "genre" => cell
                .text_color(palette::text_secondary())
                .child(SharedString::from(v.genre.to_string())),
            "year" => cell
                .text_color(palette::text_muted())
                .child(fmt_num(v.year)),
            "codec" => cell
                .text_color(palette::text_muted())
                .child(SharedString::from(v.codec.to_string())),
            "bitrate" => cell
                .text_color(palette::text_muted())
                .child(fmt_num(v.bitrate_kbps)),
            "sample_rate" => cell
                .text_color(palette::text_muted())
                .child(SharedString::from(group_head::khz(v.sample_rate_hz))),
            // Blank rather than a zero for the lossy formats, which have
            // no depth to report.
            "bit_depth" => cell
                .text_color(palette::text_muted())
                .child(fmt_num(v.bit_depth as u16)),
            "duration" => cell
                .text_color(palette::text_muted())
                .child(SharedString::from(fmt_ms(v.duration_ms))),
            // The gain the leveling would read, signed so a boost reads as
            // one, and blank for a file with neither figure rather than
            // a 0.00 that would look like a levelled track. The tag as
            // written: the preamp and the peak clamp apply at playback, and
            // folding them in here would turn a file's own number into one
            // that moves when a slider does.
            "gain" => match projection
                .gain_db(row, crate::settings::gain_mode() == GainModeSetting::Album)
            {
                Some(db) => {
                    // The old format! used ":+" to force the sign; the locale
                    // formatter has no such flag, so it's glued on by hand.
                    let sign = if db.is_sign_negative() { "-" } else { "+" };
                    let magnitude = rox_i18n::format::format_float(f64::from(db.abs()), 2);
                    cell.text_color(palette::text_muted())
                        .child(SharedString::from(format!("{sign}{magnitude}")))
                }
                None => cell,
            },
            // Whole beats a minute: the fraction under them comes from the
            // estimator rather than anything a listener counts,
            // and a column of 128.37s reads as noise. Blank for a track
            // with no tempo from either source.
            "bpm" => match v.bpm {
                Some(bpm) => cell
                    .text_color(palette::text_muted())
                    .child(SharedString::from(rox_i18n::format::format_int(
                        bpm.round() as i64,
                    ))),
                None => cell,
            },
            "rating" => {
                track_cells::rating(self.state.clone(), projection.db_id[row as usize], v.rating)
            }
            "favourite" => {
                let id = projection.db_id[row as usize];
                track_cells::favourite(self.state.clone(), id, self.favourites.contains(&id))
            }
            // The raw cosine against the playing track, two decimals, because
            // this column is for judging the vectors rather than for reading
            // as a percentage. Blank for the playing track itself, for a
            // track with no vector yet, and while nothing is playing.
            "similar" => match self.similar.get(&projection.db_id[row as usize]) {
                Some(score) => cell
                    .text_color(palette::text_muted())
                    .child(SharedString::from(rox_i18n::format::format_float(
                        f64::from(*score),
                        2,
                    ))),
                None => cell,
            },
            // Blank at zero like the track and year cells: never played
            // reads cleaner as absence than as a column of zeros. The
            // compact face shrinks the count and hangs a faint bar right
            // beside it, CaTRoX's "1|" playlist tick.
            "plays" if self.compact_plays => cell
                .flex()
                .flex_row()
                .items_center()
                .justify_end()
                .gap(px(1.))
                .when(v.plays > 0, |d| {
                    d.child(div().text_xs().text_color(palette::text_muted()).child(
                        SharedString::from(rox_i18n::format::format_int(v.plays as i64)),
                    ))
                    .child(div().text_xs().text_color(palette::text_faint()).child("|"))
                }),
            "plays" => cell
                .text_color(palette::text_muted())
                .child(if v.plays == 0 {
                    SharedString::default()
                } else {
                    SharedString::from(rox_i18n::format::format_int(v.plays as i64))
                }),
            // How long ago the track was scanned in, blank when unknown
            // (a library indexed before the timestamp existed).
            "added" => cell
                .text_color(palette::text_muted())
                .child(if v.added <= 0 {
                    SharedString::default()
                } else {
                    SharedString::from(fmt_ago(self.added_now() - v.added))
                }),
            _ => cell,
        };
        // The text's line box comes off the font (gpui's phi line height),
        // not the row, and a cell lays it from the top: at short row
        // heights the glyphs hug the cell bottom and the descenders get
        // chopped. Centering the content in the cell splits any overshoot
        // evenly, so every row height keeps its text in the middle.
        div()
            .size_full()
            .flex()
            .flex_row()
            .items_center()
            .child(cell.w_full().min_w_0())
            .into_any_element()
    }

    /// Keep the delegate's columns in the widget's order: the table calls
    /// this before it reorders its own col_groups the same way, so cell
    /// rendering (indexed by the visual column) stays aligned. The layout
    /// dump reads the new order back off `columns`.
    fn move_column(
        &mut self,
        col_ix: usize,
        to_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        if col_ix >= self.columns.len() || to_ix >= self.columns.len() {
            return;
        }
        let column = self.columns.remove(col_ix);
        self.columns.insert(to_ix, column);
    }

    /// No rows and a non-empty query means no hits; keep the body quiet
    /// like the old flat list did. The no-library case never gets here,
    /// the panel renders its own empty state instead of the table.
    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }
}

/// One browse view over the shared catalog: its own search query and row
/// order, duplicable and poppable like any panel.
pub struct LibraryPanel {
    state: AppState,
    /// The table over the current view; the delegate holds the rows.
    table: Entity<TableState<TrackTable>>,
    query: String,
    /// The panel's own focus, what the dock focuses on tab activation. Kept
    /// apart from the search input's focus so activating the tab doesn't
    /// put every keystroke in the query, and so the playback key bindings
    /// (scoped out of SearchInput) stay live.
    focus: FocusHandle,
    /// The query editor, the shared search box; `query` tracks its value
    /// via change events.
    search: Entity<SearchBox>,
    /// Show the search box; while hidden the query keeps its text but
    /// stops applying.
    show_search: bool,
    /// Filter by the panel's own `query` or follow the shared app-wide one.
    /// While global the box shows and writes the shared query; `query`
    /// keeps the panel's own text, dormant, for the switch back to local.
    query_source: QuerySource,
    /// A pending box reset: the active source's text needs to go into the
    /// box, but that needs a window, so the next render (which has one)
    /// applies it. Set on a source toggle or a shared-query change.
    resync_box: bool,
    /// The tracks this panel is pinned to while following the selection.
    /// Runtime only: a restore re-pins from whatever is picked then.
    selection_ids: Vec<i64>,
    /// A panel-local error (a failed play), shown until the catalog updates.
    error: Option<SharedString>,
    /// The playing track's path, the change detector: the player notifies
    /// every pump tick, so everything up to this compare stays cheap.
    playing_key: Option<TrackKey>,
    /// The type-ahead buffer and when it last grew; a pause starts over.
    type_ahead: String,
    type_ahead_at: Option<std::time::Instant>,
    /// The saved scroll row waiting for rows to restore against. The
    /// catalog loads after the panel builds, so the first non-empty view
    /// consumes this; None once applied.
    restore_scroll: Option<usize>,
    /// Scroll to the playing row when the track changes, and whether to
    /// glide there instead of jumping.
    follow_playing: bool,
    smooth_follow: bool,
    /// The row the last follow aimed at, so a catalog refresh that leaves
    /// the playing track where it already was doesn't scroll there again.
    followed_row: Option<usize>,
    /// Whether the next view to land should catch the follow up. Set by the
    /// catalog change that asked for the rebuild, since the playing row it
    /// wants to scroll to only exists once that view is installed.
    follow_on_view: bool,
    /// Scroll back to the playing row on its own once the list has gone
    /// untouched a spell.
    resume_playing: bool,
    /// The idle-resume clock: stamped on every scroll or press, it wakes
    /// the list back to the playing row when `resume_playing` is on and the
    /// user has stepped away.
    resume_idle: ResumeIdle,
    /// The view row the follow glide is headed to; stepped every frame in
    /// `body` and cleared on arrival.
    glide_to: Option<usize>,
    /// The last glide tick, its dt.
    glide_tick: Instant,
    /// The track rows' height in px at the stock font size, applied on
    /// the table each render, and one header line's height, free of it.
    /// The delegate keeps a copy of both for the block math.
    row_height: f32,
    head_height: f32,
    /// Extra height grown into each row, which the row fills; the table
    /// option holds it and the delegate copies it for the block math.
    row_spacing: f32,
    /// The header lines' text size, free of the line height; the delegate
    /// copies it for the header renders.
    head_text: f32,
    /// The height sliders' scrub strips, for the settings window.
    row_scrub: ScrubState,
    head_scrub: ScrubState,
    row_spacing_scrub: ScrubState,
    head_text_scrub: ScrubState,
    /// The header style and what the headers group on. The delegate
    /// copies both for the view computation; they're kept here too so the
    /// dropdown's checkmarks build without reading the table entity
    /// (the row context menu builds mid-table-update).
    headers: Headers,
    group_by: GroupBy,
    /// The keys of the currently shown columns, copied off the delegate
    /// whenever the set changes so the Columns dropdown builds its checks
    /// without reading the table entity (the row context menu builds
    /// mid-table-update). Order and width are stored on the delegate; only
    /// the shown set matters here.
    columns_shown: HashSet<String>,
    /// The acoustic model and whether it has described anything, as of the
    /// last look. Both are process statics rather than entities, so there's
    /// nothing to subscribe to; both repaint every window when they move,
    /// which brings [`LibraryPanel::watch_similarity`] round to notice.
    /// Compared rather than acted on, so an idle frame costs two reads and
    /// no work.
    similar_watch: (String, bool),
    /// The header tiles' corner radius; the delegate copies it for the
    /// tile render, and the config dump stores it.
    art_rounding: f32,
    /// The art rounding slider's scrub strip, for the settings window.
    art_scrub: ScrubState,
    /// Which side the header blocks' cover tile sits on, the rounding's
    /// route.
    art_side: ArtSide,
    /// The tile's inset from the block edges, and its slider's strip.
    art_margin: f32,
    art_margin_scrub: ScrubState,
    /// The open space over and under each header block, and their strips.
    header_gap_above: f32,
    header_gap_above_scrub: ScrubState,
    header_gap_below: f32,
    header_gap_below_scrub: ScrubState,
    /// The one readout being typed into across the settings sliders.
    value_edit: panel::ValueEdit,
    /// The header rows' cover tile knob; the delegate copies it for the
    /// header renders, and the config dump stores it.
    header_art: bool,
    /// The artist grouping's full-circle tiles, the wall's face; same
    /// route as the tile knob.
    portrait_circle: bool,
    /// The genre grouping's tile face, the grid's looks; same route.
    genre_face: TileFace,
    /// Header rows on the list background instead of the Elevated tint,
    /// same route.
    header_flush: bool,
    /// The compact header's composed row, the arrange editor's list.
    header_compact: Vec<HeadPiece>,
    /// The expanded block's composed lines, always [`HEAD_LINE_SLOTS`]
    /// entries, the editor's slots; an empty slot drops out of the
    /// rendered block. The delegate copies the current mode's effective
    /// lines, and the config dump stores these as saved.
    header_lines: Vec<Vec<HeadPiece>>,
    /// How many line wells the rows editor holds open. The fixed slots
    /// can't say "added but still empty", so this UI-only count keeps a
    /// fresh well up past the last filled slot; not persisted.
    header_lines_shown: usize,
    /// The plays column's compact face: a small count and a faint dash
    /// instead of the plain number.
    compact_plays: bool,
    /// Tint every other track row; read at render, stored in the dump.
    stripes: bool,
    /// Draw the hairline under each track row, same route as the stripes.
    row_borders: bool,
    /// Draw the column header row over the list, same route again.
    column_headers: bool,
    /// Sort on a plain click in the header instead of on the sort icon.
    /// The delegate keeps a copy for the header it draws, and the widget's
    /// own sorting and its always-on column drag follow this one.
    sort_on_click: bool,
    /// The open column rename window and the key it renames, if any:
    /// opening a rename on the same header focuses it rather than
    /// stacking a second dialog, and one on another header closes it
    /// first. A closed window leaves a handle whose activate fails, so
    /// the next open falls through and replaces it.
    column_rename: Option<(String, WindowHandle<Root>)>,
    /// The rename, theme override, and placement locks shared by every
    /// panel, live for the render and stored in the config dump like
    /// every other view knob.
    chrome: PanelChrome,
    /// The tab panel this panel is currently in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Watches the hosting tab panel: whether this panel is solo decides
    /// where the toolbar renders, so membership changes must re-render.
    _tabs_changed: Option<Subscription>,
    _library_changed: Subscription,
    _table_events: Subscription,
    _search_events: Subscription,
    _query_changed: Subscription,
    _selection_changed: Subscription,
    _player_changed: Subscription,
    _thumbs_changed: Subscription,
    /// Watches the portrait service for the artist grouping's header
    /// tiles, the artist wall's move: an arriving face repaints the rows.
    _portraits_changed: Subscription,
    /// Drops the phrase when focus leaves the panel, so tab goes back to
    /// walking panels instead of cycling a phrase from a past visit.
    _type_ahead_blur: Subscription,
}

impl LibraryPanel {
    pub fn new(
        state: AppState,
        config: LibraryConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut LibraryPanel, _, event: &LibraryEvent, cx| {
                // A rating click or a recorded listen only needs the cells
                // repainted: the value is in the shared projection
                // already, and re-sorting a rating-sorted view here would
                // yank the row out from under the cursor mid-click. The
                // order catches up on the next refresh.
                if matches!(event, LibraryEvent::Rated | LibraryEvent::Played) {
                    this.table.update(cx, |_, cx| cx.notify());
                    return;
                }
                // A playlist edit doesn't touch the catalog view, only the
                // favourite highlights: reload the set and repaint, no rebuild.
                if matches!(event, LibraryEvent::PlaylistsChanged) {
                    this.reload_favourites(cx);
                    return;
                }
                this.error = None;
                // The catalog loads after a restored track starts, so the
                // launch's follow waits for this first rebuild; a rescan
                // that moves the playing row re-scrolls the same way. The
                // catch-up runs when the rows land, in `on_view_installed`.
                this.follow_on_view = true;
                this.refresh_view(cx);
                cx.notify();
                this.refresh_title_bar(cx);
            },
        );
        // The saved composition, or the stock lines with the legacy year
        // and details toggles folded in; same fold for the heights and
        // the old density.
        let (header_compact, header_lines) = fold_head_lines(&config);
        // The editor opens wells through the last filled slot, one at least.
        let header_lines_shown = header_lines
            .iter()
            .rposition(|line| !line.is_empty())
            .map_or(1, |last| last + 1);
        let (row_height, head_height) = fold_row_heights(&config);
        let art_margin = fold_margin(config.art_margin, ART_MARGIN_MAX);
        let header_gap_above = fold_margin(config.header_gap_above, HEAD_GAP_MAX);
        let header_gap_below = fold_margin(config.header_gap_below, HEAD_GAP_MAX);
        // A layout written before the header gates could name a column that
        // sorts on nothing. Dropping it here falls back to the canonical
        // order the panel would have drawn anyway, and the next save writes
        // the truth instead of keeping the dead key.
        let sort = config
            .sort_key
            .filter(|key| columns::sortable(key))
            .map(|key| (SharedString::from(key), config.sort_desc));
        // The renamed headers ride the same layout the columns come from.
        let labels = columns::label_overrides(&config.column_layout);
        let delegate = TrackTable {
            state: state.clone(),
            panel: cx.weak_entity(),
            view: Arc::new(Vec::new()),
            groups: Vec::new(),
            headers: config.headers,
            group_by: config.group_by,
            row_height,
            row_spacing: fold_margin(config.row_spacing, ROW_SPACING_MAX),
            head_height,
            head_text: fold_head_text(config.head_text),
            art_rounding: config.art_rounding,
            art_side: config.art_side,
            art_margin,
            header_gap_above,
            header_gap_below,
            header_art: config.header_art,
            portrait_circle: config.portrait_circle,
            genre_face: config.genre_face,
            header_flush: config.header_flush,
            head_lines: effective_head_lines(config.headers, &header_compact, &header_lines),
            compact_plays: config.compact_plays,
            selected: HashSet::new(),
            anchor: None,
            cursor: None,
            columns: track_columns(&config.column_layout, &sort, &labels),
            labels,
            sort_on_click: config.sort_on_click,
            columns_locale: rox_i18n::locale(),
            sort,
            playing_id: None,
            playing_row: None,
            favourites: state.library.read(cx).favourite_ids(),
            similar: Arc::new(HashMap::new()),
            similar_anchor: None,
            cover_paths: HashMap::new(),
            drag_keys: HashMap::new(),
            sel_gen: 0,
            view_gen: 0,
            added_now: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            added_now_at: Instant::now(),
            drag_set: None,
        };
        // Widths and order persist by column key, so a drag is kept across a
        // layout save; the delegate copies the widget's reorder.
        // Click-to-sort takes the widget's own sorting off (the header
        // click runs the cycle instead) and takes the column drag over to
        // Alt, which `set_alt` flips as the modifier comes and goes.
        let table = cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .col_movable(!config.sort_on_click)
                .sortable(!config.sort_on_click)
                .col_selectable(false)
        });
        let _table_events = cx.subscribe_in(&table, window, Self::on_table_event);
        // A panel restored as global opens showing the shared query; a local
        // one shows its own.
        let initial = match config.query_source {
            QuerySource::Global => state.query.read(cx).text().to_string(),
            QuerySource::Local | QuerySource::Selection => config.query.clone(),
        };
        let search =
            cx.new(|cx| SearchBox::new(rox_i18n::t!("query-search"), &initial, window, cx).small());
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        // Follow the shared query while global: re-filter and reset the box
        // to it on the next render. The reset needs a window, so it goes
        // through the resync flag rather than happening here.
        let _query_changed = cx.subscribe(
            &state.query,
            |this: &mut LibraryPanel, _, _: &SharedQueryEvent, cx| {
                this.on_shared_query_changed(cx);
            },
        );
        // Restored as selection-following, the table opens on whatever is
        // picked now, rather than blank until the next pick.
        let selection_ids = state.selection.read(cx).tracks().to_vec();
        // Follow the app-wide selection while pinned to it.
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, event: &SelectionEvent, cx| {
                this.on_selection_changed(event.source, cx);
            },
        );
        let _player_changed = cx.observe(&state.player, |this: &mut LibraryPanel, _, cx| {
            this.sync_playing(cx)
        });
        // An arriving thumbnail or portrait repaints the rows; the panel
        // itself has nothing to recompute.
        let _thumbs_changed = cx.observe(&state.thumbs, |this: &mut LibraryPanel, _, cx| {
            this.table.update(cx, |_, cx| cx.notify());
        });
        let _portraits_changed = cx.observe(&state.portraits, |this: &mut LibraryPanel, _, cx| {
            this.table.update(cx, |_, cx| cx.notify());
        });
        let focus = cx.focus_handle().tab_stop(true);
        // The phrase outlives its badge, so it needs an end: leaving the
        // panel drops it, which is also what hands tab back to traversal.
        let panel = cx.weak_entity();
        let _type_ahead_blur = window.on_focus_out(&focus, cx, move |_, _, cx| {
            panel
                .update(cx, |this: &mut LibraryPanel, cx| {
                    this.clear_type_ahead(cx);
                })
                .ok();
        });
        let mut this = LibraryPanel {
            state,
            table,
            query: config.query,
            focus,
            search,
            show_search: config.search,
            query_source: config.query_source,
            resync_box: false,
            selection_ids,
            error: None,
            playing_key: None,
            type_ahead: String::new(),
            type_ahead_at: None,
            restore_scroll: (config.scroll_row > 0).then_some(config.scroll_row),
            follow_playing: config.follow_playing,
            smooth_follow: config.smooth_follow,
            followed_row: None,
            follow_on_view: false,
            resume_playing: config.resume_playing,
            resume_idle: ResumeIdle::default(),
            glide_to: None,
            glide_tick: Instant::now(),
            row_height,
            head_height,
            row_spacing: fold_margin(config.row_spacing, ROW_SPACING_MAX),
            head_text: fold_head_text(config.head_text),
            row_scrub: ScrubState::default(),
            head_scrub: ScrubState::default(),
            row_spacing_scrub: ScrubState::default(),
            head_text_scrub: ScrubState::default(),
            headers: config.headers,
            group_by: config.group_by,
            columns_shown: HashSet::new(),
            similar_watch: (
                crate::settings::acoustic_source().id().to_string(),
                crate::settings::similarity_ready(),
            ),
            art_rounding: config.art_rounding,
            art_scrub: ScrubState::default(),
            art_side: config.art_side,
            art_margin,
            art_margin_scrub: ScrubState::default(),
            header_gap_above,
            header_gap_above_scrub: ScrubState::default(),
            header_gap_below,
            header_gap_below_scrub: ScrubState::default(),
            value_edit: panel::ValueEdit::default(),
            header_art: config.header_art,
            portrait_circle: config.portrait_circle,
            genre_face: config.genre_face,
            header_flush: config.header_flush,
            header_compact,
            header_lines,
            header_lines_shown,
            compact_plays: config.compact_plays,
            stripes: config.stripes,
            row_borders: config.row_borders,
            column_headers: config.column_headers,
            sort_on_click: config.sort_on_click,
            column_rename: None,
            chrome: config.chrome,
            tab_panel: None,
            _tabs_changed: None,
            _library_changed,
            _table_events,
            _search_events,
            _query_changed,
            _selection_changed,
            _player_changed,
            _thumbs_changed,
            _portraits_changed,
            _type_ahead_blur,
        };
        this.refresh_view(cx);
        this.columns_shown = this.shown_columns(cx);
        // A duplicate opens with a track already playing; pick it up now
        // instead of waiting for the next track change.
        this.sync_playing(cx);
        this
    }

    /// Follow the player: on a track change, resolve the playing path to
    /// its id (one store lookup) and re-locate its row in the view.
    fn sync_playing(&mut self, cx: &mut Context<Self>) {
        let path = self.state.player.read(cx).now_playing().map(|now| now.key);
        if path == self.playing_key {
            return;
        }
        self.playing_key = path;
        let id = self
            .playing_key
            .as_ref()
            .and_then(|key| self.state.library.read(cx).id_for_key(key));
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.playing_id = id;
            delegate.locate_playing(cx);
            cx.notify();
        });
        if self.follow_playing {
            self.follow_playing(cx);
        }
        self.refresh_similarity(cx);
    }

    /// Catch the two things that stale the Similar scores without the
    /// playing track moving: a switched extractor, which keys a different
    /// set of vectors, and an analysis pass arriving where there was nothing
    /// to rank before. Neither is an entity, so neither can be subscribed
    /// to; both repaint every window when they change, so the render path
    /// compares them instead. Edge-triggered, since the scan behind this
    /// belongs on a track change, never on a frame.
    fn watch_similarity(&mut self, cx: &mut Context<Self>) {
        let model = crate::settings::acoustic_source();
        let ready = crate::settings::similarity_ready();
        if self.similar_watch.0 == model.id() && self.similar_watch.1 == ready {
            return;
        }
        self.similar_watch = (model.id().to_string(), ready);
        self.refresh_similarity(cx);
    }

    /// Rescore the library against the playing track for the Similar column.
    ///
    /// The raw cosine, `embeddings::scores`, not the ranking playback draws
    /// from. This column is a look at the vectors, so a number here that had
    /// been marked down for the track's tempo would read as the model hearing
    /// something it didn't.
    ///
    /// Off the UI thread on its own connection, the ReplayGain pass's move.
    /// The store keeps the standardized corpus in memory, so a track change
    /// costs a dot product per track, ten milliseconds or so on a
    /// fifty-thousand-track library. What it can cost is the first question
    /// after the analysis pass writes something: that one rereads every
    /// vector, a few hundred milliseconds, which is exactly why this doesn't
    /// happen on the UI thread. A seed the transport already drew against
    /// costs nothing at all, since the store holds the last few seeds'
    /// scores. Skipped entirely while the column isn't shown, so a panel
    /// without it pays nothing.
    fn refresh_similarity(&mut self, cx: &mut Context<Self>) {
        if !self.shown_columns(cx).contains("similar") {
            return;
        }
        // Whichever model the Library page has selected, so the column shows
        // distances under the same model the analysis pass filled. It's half
        // the key the scores are held under: switching extractors leaves the
        // old model's numbers on screen otherwise.
        let model = crate::settings::acoustic_source().id().to_string();
        let delegate = self.table.read(cx).delegate();
        let anchor = delegate.playing_id;
        if delegate
            .similar_anchor
            .as_ref()
            .is_some_and(|(id, under)| Some(*id) == anchor && *under == model)
        {
            return;
        }
        let Some(anchor) = anchor else {
            // Nothing playing: drop the scores rather than leaving the
            // column showing distances to a track that stopped. Already
            // empty is the common case here, and repainting for it would
            // be a frame spent on nothing.
            self.table.update(cx, |table, cx| {
                let delegate = table.delegate_mut();
                if delegate.similar.is_empty() {
                    return;
                }
                delegate.similar = Arc::new(HashMap::new());
                delegate.similar_anchor = None;
                cx.notify();
            });
            return;
        };
        let db_path = self.state.library.read(cx).db_path();
        let scoring = model.clone();
        cx.spawn(async move |this, cx| {
            let scored = cx
                .background_executor()
                .spawn(async move {
                    let conn = rox_library::store::open(&db_path).ok()?;
                    rox_library::embeddings::scores(&conn, anchor, &scoring).ok()
                })
                .await;
            let Some(scored) = scored else { return };
            this.update(cx, |this, cx| {
                this.table.update(cx, |table, cx| {
                    let delegate = table.delegate_mut();
                    // An empty result is a corpus this model hasn't described
                    // yet rather than a real score: leave the stamp off so a
                    // later pass gets scored instead of this standing as the
                    // last word on the track.
                    delegate.similar_anchor = (!scored.is_empty()).then_some((anchor, model));
                    delegate.similar = Arc::new(scored.into_iter().collect());
                    cx.notify();
                });
                // A view ordered by similarity is now ordered by the old
                // track's scores, so it has to be rebuilt against the new.
                let sorted_by_similarity = this
                    .table
                    .read(cx)
                    .delegate()
                    .sort
                    .as_ref()
                    .is_some_and(|(key, _)| key.as_ref() == "similar");
                if sorted_by_similarity {
                    this.refresh_view(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Scroll the playing row into view: a glide when smooth is on, the
    /// jump otherwise. Scroll only: the automatic follow never touches
    /// the selection, that's the menu jump's move.
    fn follow_playing(&mut self, cx: &mut Context<Self>) {
        self.followed_row = self.table.read(cx).delegate().playing_row;
        if self.smooth_follow {
            if let Some(row) = self.table.read(cx).delegate().playing_row {
                self.glide_to = Some(row);
                cx.notify();
            }
        } else {
            self.table.update(cx, |table, cx| {
                if let Some(row) = table.delegate().playing_row {
                    table.scroll_to_row(row, cx);
                }
            });
        }
    }

    /// A scroll, drag, or press: restart the idle clock and arm a wake, so
    /// the list scrolls back to the playing row once the user steps away. A
    /// no-op unless the resume behavior is on, so an off panel spends
    /// nothing per gesture.
    fn touch_resume(&mut self, cx: &mut Context<Self>) {
        if self.resume_playing {
            self.resume_idle.touch(cx, Self::resume_to_playing);
        }
    }

    /// What the idle wake does: scroll back to the playing row, so long as
    /// the resume is still on. The clock only fires this once the list has
    /// gone untouched a full window, a gesture in between having pushed it
    /// out, so no extra idle check is needed here.
    fn resume_to_playing(&mut self, cx: &mut Context<Self>) {
        if self.resume_playing {
            self.follow_playing(cx);
        }
    }

    /// Browse from the keyboard while the panel itself is focused: arrows
    /// move a cursor, shift extends from the click path's anchor, enter
    /// plays, and plain typing jumps to the next match in the leading
    /// column. With the search box focused these stay out of the way: in
    /// the solo and popped-out layouts its toolbar is inside the panel
    /// root, so its keystrokes bubble through here.
    fn on_panel_key(&mut self, event: &KeyDownEvent, window: &Window, cx: &mut Context<Self>) {
        if self.search.read(cx).is_focused(window, cx) {
            return;
        }
        let keystroke = &event.keystroke;
        // Select-all uses the platform chord, so it goes before the
        // modifier bail below.
        if keystroke.modifiers.secondary() && keystroke.key.as_str() == "a" {
            self.select_all(cx);
            return;
        }
        if keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.alt {
            return;
        }
        // Arrow and type-ahead navigation is browsing too, so it restarts
        // the idle clock the same as a scroll or a click.
        self.touch_resume(cx);
        let shift = keystroke.modifiers.shift;
        match keystroke.key.as_str() {
            // The escape ladder: a phrase drops first, since it's
            // holding tab, then the selection.
            "escape" => {
                if !self.clear_type_ahead(cx) {
                    self.deselect(cx);
                }
            }
            "up" => self.move_cursor(-1, shift, cx),
            "down" => self.move_cursor(1, shift, cx),
            "pageup" => self.move_cursor(-PAGE_ROWS, shift, cx),
            "pagedown" => self.move_cursor(PAGE_ROWS, shift, cx),
            // The edges snap inward past a leading header.
            "home" => {
                if let Some(ix) = self.table.read(cx).delegate().snap_to_track(0, true) {
                    self.set_cursor(ix, shift, cx);
                }
            }
            "end" => {
                let target = {
                    let delegate = self.table.read(cx).delegate();
                    delegate.snap_to_track(delegate.view.len().saturating_sub(1), false)
                };
                if let Some(ix) = target {
                    self.set_cursor(ix, shift, cx);
                }
            }
            "enter" => self.play_selection(cx),
            _ => {
                let Some(text) = &keystroke.key_char else {
                    return;
                };
                // Space stays the workspace's play/pause; it never starts
                // a jump, only continues one mid-phrase.
                if text == " " && !panel::type_ahead_live(self.type_ahead_at) {
                    return;
                }
                // Consumed as type-ahead text: stop it here so it doesn't
                // also match the workspace's space-bound TogglePlayback
                // binding, which this panel otherwise inherits unscoped.
                cx.stop_propagation();
                self.type_to(text.clone(), cx);
            }
        }
    }

    /// Grow or restart the type-ahead buffer and jump to its next match.
    /// A grown buffer re-tests the current row first, so refining a match
    /// stays put instead of skipping ahead.
    fn type_to(&mut self, text: String, cx: &mut Context<Self>) {
        let grown = panel::type_ahead_grow(&mut self.type_ahead, &mut self.type_ahead_at, text);
        // The badge shows the phrase now and leaves when the window
        // lapses; a miss below still updated it, so repaint either way.
        panel::type_ahead_fade(cx);
        cx.notify();
        let target = {
            let delegate = self.table.read(cx).delegate();
            delegate.find_prefix(&self.type_ahead, grown, cx)
        };
        if let Some(ix) = target {
            self.set_cursor(ix, false, cx);
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
    /// stamp alone: the badge and the letter grouping belong to typing,
    /// so a run of tabs steps silently rather than reviving them.
    fn type_step(&mut self, back: bool, cx: &mut Context<Self>) {
        if self.type_ahead.is_empty() {
            return;
        }
        cx.notify();
        let target = {
            let delegate = self.table.read(cx).delegate();
            delegate.find_step(&self.type_ahead, back, cx)
        };
        if let Some(ix) = target {
            self.set_cursor(ix, false, cx);
        }
    }

    /// The type-ahead badge text: a `field:` pin shows as the column's
    /// label in parens ahead of what's matching, so `artist:bea` reads
    /// `(Artist) bea`; a plain phrase, matching any naming field, shows
    /// bare. Empty while there's no phrase, same as the bare buffer, so
    /// [`panel::type_ahead_overlay`]'s own emptiness check still hides
    /// the badge.
    fn type_ahead_display(&self) -> String {
        if let Some((field, needle)) = TrackTable::type_ahead_pin(&self.type_ahead) {
            if let Some(column) = columns::columns().iter().find(|c| c.key == field) {
                return format!("({}) {}", column.label, needle);
            }
        }
        self.type_ahead.clone()
    }

    /// Ctrl/Cmd+A: every track row of the current view, headers and disc
    /// dividers skipped, anchored at the top.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            let all: HashSet<usize> = (0..delegate.view.len())
                .filter(|&i| delegate.track_at(i).is_some())
                .collect();
            if all.is_empty() {
                return;
            }
            delegate.anchor = all.iter().copied().min();
            delegate.selected = all;
            delegate.sel_gen += 1;
            table.delegate().publish_selection(cx);
            cx.notify();
        });
    }

    /// Escape drops the selection, handing the shared scope back to the
    /// whole catalog.
    fn deselect(&mut self, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            table.clear_selection(cx);
            let delegate = table.delegate_mut();
            if delegate.selected.is_empty() {
                return;
            }
            delegate.selected.clear();
            delegate.anchor = None;
            delegate.cursor = None;
            delegate.sel_gen += 1;
            table.delegate().publish_selection(cx);
            cx.notify();
        });
    }

    /// Put the cursor on a view row: plain selects just it, extend grows
    /// the selection from the anchor. Either way it publishes and scrolls
    /// into view.
    fn set_cursor(&mut self, ix: usize, extend: bool, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            if delegate.track_at(ix).is_none() {
                return;
            }
            delegate.cursor = Some(ix);
            if extend {
                let anchor = delegate.anchor.unwrap_or(ix);
                let (lo, hi) = (anchor.min(ix), anchor.max(ix));
                // A range spanning a group break selects its tracks only.
                let range = (lo..=hi)
                    .filter(|&i| delegate.track_at(i).is_some())
                    .collect();
                delegate.selected = range;
                if delegate.anchor.is_none() {
                    delegate.anchor = Some(anchor);
                }
            } else {
                delegate.selected = HashSet::from([ix]);
                delegate.anchor = Some(ix);
            }
            delegate.sel_gen += 1;
            table.delegate().publish_selection(cx);
            table.scroll_to_row(ix, cx);
            cx.notify();
        });
    }

    /// Step the cursor; the first press with no cursor starts at the edge
    /// the step heads toward. A step that hits a header overshoots it the
    /// way it was heading, bouncing back at the ends.
    fn move_cursor(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let target = {
            let delegate = self.table.read(cx).delegate();
            let len = delegate.view.len();
            if len == 0 {
                return;
            }
            let raw = match delegate.cursor {
                None if delta >= 0 => 0,
                None => len - 1,
                Some(cursor) => (cursor as isize + delta).clamp(0, len as isize - 1) as usize,
            };
            delegate.snap_to_track(raw, delta >= 0)
        };
        if let Some(target) = target {
            self.set_cursor(target, extend, cx);
        }
    }

    /// Enter: a multi-selection plays exactly itself, a lone cursor plays
    /// from its row in view order like a double click.
    fn play_selection(&mut self, cx: &mut Context<Self>) {
        let (mut rows, cursor) = {
            let delegate = self.table.read(cx).delegate();
            let rows: Vec<usize> = delegate.selected.iter().copied().collect();
            (rows, delegate.cursor)
        };
        rows.sort_unstable();
        if rows.len() > 1 {
            self.play_rows(rows, cx);
        } else if let Some(ix) = cursor.or_else(|| rows.first().copied()) {
            self.play_from(ix, cx);
        }
    }

    /// The menu's jump: put the cursor on the playing row, which selects
    /// it, publishes, and scrolls it into view in one move.
    fn jump_to_playing(&mut self, cx: &mut Context<Self>) {
        let row = self.table.read(cx).delegate().playing_row;
        if let Some(row) = row {
            self.set_cursor(row, false, cx);
        }
    }

    /// The menu's follow toggle: flip the follow state and catch up right
    /// away when turning it on, the same move as the settings switch.
    fn toggle_follow_playing(&mut self, cx: &mut Context<Self>) {
        self.follow_playing = !self.follow_playing;
        if self.follow_playing {
            self.follow_playing(cx);
        }
        cx.notify();
    }

    /// Re-read the favourited set into the table and repaint the hearts. Runs
    /// on a playlist change, far cheaper than a view rebuild since the rows
    /// themselves do not move.
    fn reload_favourites(&mut self, cx: &mut Context<Self>) {
        let favourites = self.state.library.read(cx).favourite_ids();
        self.table.update(cx, |table, cx| {
            table.delegate_mut().favourites = favourites;
            cx.notify();
        });
    }

    /// Rebuild the rows for the current query, filter, and sort. The pass
    /// itself runs on the background executor: a search over ten million
    /// rows is tens of milliseconds and the sort behind it can be near a
    /// second, which is a dropped frame either way if it runs here. The
    /// old rows stay on screen until the new ones land.
    fn refresh_view(&mut self, cx: &mut Context<Self>) {
        self.schedule_view(false, cx);
    }

    /// [`Self::refresh_view`] behind the keystroke debounce: typing into a
    /// search box fires one of these per character, and a pass started per
    /// keystroke is work thrown away by the next one. The generation check
    /// already makes stale results harmless; the wait keeps them from being
    /// started at all.
    fn refresh_view_debounced(&mut self, cx: &mut Context<Self>) {
        self.schedule_view(true, cx);
    }

    fn schedule_view(&mut self, debounce: bool, cx: &mut Context<Self>) {
        let query = self.effective_query(cx);
        let filter = self.effective_filter(cx);
        let generation = self.table.update(cx, |table, _| {
            let delegate = table.delegate_mut();
            delegate.view_gen += 1;
            delegate.view_gen
        });
        let inputs = self
            .table
            .read(cx)
            .delegate()
            .view_inputs(&query, &filter, cx);
        let Some(inputs) = inputs else {
            // No projection yet: install the empty view straight away, so a
            // panel built before the catalog loads shows nothing rather
            // than whatever it held before.
            self.table.update(cx, |table, cx| {
                install_view(table, generation, Arc::new(Vec::new()), Vec::new(), cx);
            });
            self.on_view_installed(cx);
            return;
        };
        cx.spawn(async move |this, cx| {
            if debounce {
                cx.background_executor().timer(VIEW_DEBOUNCE).await;
                // Another keystroke landed while this one waited, so its
                // pass is the one worth running.
                let live = this
                    .update(cx, |this, cx| {
                        this.table.read(cx).delegate().view_gen == generation
                    })
                    .unwrap_or(false);
                if !live {
                    return;
                }
            }
            let (view, groups) = cx
                .background_executor()
                .spawn(async move { compute_rows(&inputs) })
                .await;
            this.update(cx, |this, cx| {
                let installed = this.table.update(cx, |table, cx| {
                    install_view(table, generation, view, groups, cx)
                });
                if installed {
                    this.on_view_installed(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// What used to run under `refresh_view` once the rows were in hand,
    /// now that they arrive a frame or more later.
    fn on_view_installed(&mut self, cx: &mut Context<Self>) {
        // The saved scroll restores against the first view with rows; a
        // strict deferred scroll on the handle, so it runs on the paint
        // that shows them, even if the panel is in a background tab
        // until then. Earlier refreshes (the empty initial load) keep it
        // pending.
        if let Some(row) = self.restore_scroll {
            if !self.table.read(cx).delegate().view.is_empty() {
                self.restore_scroll = None;
                self.table
                    .read(cx)
                    .vertical_scroll_handle
                    .scroll_to_item_strict(row, ScrollStrategy::Top);
            }
        }
        // The catalog load's follow waited for the rows, so it runs here:
        // the playing row's index only exists once the view holding it is
        // installed. A refresh that leaves the row where it was does not
        // re-scroll, so a tag save that reindexes a few files can't yank
        // the list off whatever was being read.
        if std::mem::take(&mut self.follow_on_view)
            && self.follow_playing
            && self.table.read(cx).delegate().playing_row != self.followed_row
        {
            self.follow_playing(cx);
        }
    }

    fn on_table_event(
        &mut self,
        _: &Entity<TableState<TrackTable>>,
        event: &TableEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // A click selects; focus moves back to the panel so the
            // playback keys stay with the workspace, not the table. Shift
            // extends from the anchor, cmd (ctrl elsewhere) toggles, and a
            // plain click starts over. The widget also fires this for a
            // double click's first clicks, which read as a plain select.
            TableEvent::SelectRow(ix) => {
                window.focus(&self.focus);
                let ix = *ix;
                // A click on a group header selects its album whole, and
                // the modifiers work by album the way they do by row: cmd
                // (ctrl elsewhere) toggles the whole group in and out,
                // shift extends from the anchor across it, ctrl+shift
                // stacks that range on. The widget's own focus row drops
                // either way, so the header strip itself takes no mark;
                // disc dividers just clear.
                if self.table.read(cx).delegate().track_at(ix).is_none() {
                    let modifiers = window.modifiers();
                    self.table.update(cx, |table, cx| {
                        table.clear_selection(cx);
                        let Some(rows) = table.delegate().group_track_rows(ix) else {
                            return;
                        };
                        let (Some(&first), Some(&last)) = (rows.first(), rows.last()) else {
                            return;
                        };
                        let delegate = table.delegate_mut();
                        if modifiers.shift {
                            // The range runs from the anchor over the far
                            // edge of the group, tracks only across the
                            // breaks, like the keyboard's shift-extend.
                            let anchor = delegate.anchor.unwrap_or(first);
                            let (lo, hi) = (anchor.min(first), anchor.max(last));
                            let range: Vec<usize> = (lo..=hi)
                                .filter(|&i| delegate.track_at(i).is_some())
                                .collect();
                            if modifiers.secondary() {
                                delegate.selected.extend(range);
                            } else {
                                delegate.selected = range.into_iter().collect();
                            }
                            if delegate.anchor.is_none() {
                                delegate.anchor = Some(anchor);
                            }
                        } else if modifiers.secondary() {
                            // Fully selected toggles off; anything less
                            // completes the group in place.
                            if rows.iter().all(|r| delegate.selected.contains(r)) {
                                for r in &rows {
                                    delegate.selected.remove(r);
                                }
                            } else {
                                delegate.selected.extend(rows.iter().copied());
                            }
                            delegate.anchor = Some(first);
                        } else {
                            delegate.selected = rows.into_iter().collect();
                            delegate.anchor = Some(first);
                        }
                        delegate.cursor = Some(first);
                        delegate.sel_gen += 1;
                        table.delegate().publish_selection(cx);
                        cx.notify();
                    });
                    return;
                }
                let modifiers = window.modifiers();
                self.table.update(cx, |table, cx| {
                    let delegate = table.delegate_mut();
                    if modifiers.shift {
                        let anchor = delegate.anchor.unwrap_or(ix);
                        let (lo, hi) = (anchor.min(ix), anchor.max(ix));
                        // Tracks only across a group break, like the
                        // keyboard's shift-extend.
                        let range: Vec<usize> = (lo..=hi)
                            .filter(|&i| delegate.track_at(i).is_some())
                            .collect();
                        // Ctrl+Shift stacks the range onto the selection so
                        // you can skip a run and grab a second block; plain
                        // shift replaces.
                        if modifiers.secondary() {
                            delegate.selected.extend(range);
                        } else {
                            delegate.selected = range.into_iter().collect();
                        }
                        if delegate.anchor.is_none() {
                            delegate.anchor = Some(anchor);
                        }
                    } else if modifiers.secondary() {
                        if !delegate.selected.insert(ix) {
                            delegate.selected.remove(&ix);
                            // The widget put its focus row here on the way
                            // in; a toggle-off must clear that too.
                            table.clear_selection(cx);
                        }
                        table.delegate_mut().anchor = Some(ix);
                    } else {
                        delegate.selected = HashSet::from([ix]);
                        delegate.anchor = Some(ix);
                    }
                    table.delegate_mut().cursor = Some(ix);
                    table.delegate_mut().sel_gen += 1;
                    table.delegate().publish_selection(cx);
                    cx.notify();
                });
            }
            // The double click plays, leaving single clicks free to
            // select. A track plays from itself through the view; a
            // group header plays its album whole, the same Play Album the
            // right click offers. A disc divider plays nothing, so its
            // rows come back empty.
            TableEvent::DoubleClickedRow(ix) => {
                let ix = *ix;
                let (is_track, album) = {
                    let delegate = self.table.read(cx).delegate();
                    (
                        delegate.track_at(ix).is_some(),
                        delegate.group_track_rows(ix).unwrap_or_default(),
                    )
                };
                if is_track {
                    self.play_from(ix, cx);
                } else if !album.is_empty() {
                    self.play_rows(album, cx);
                }
            }
            // Written back into the delegate's columns: refresh() re-reads
            // them, and the save request persists them.
            TableEvent::ColumnWidthsChanged(widths) => {
                let widths = widths.clone();
                self.table.update(cx, |table, _| {
                    let columns = &mut table.delegate_mut().columns;
                    for (column, width) in columns.iter_mut().zip(widths) {
                        column.width = width;
                    }
                });
                self.request_layout_save(cx);
            }
            // The widget already reordered the delegate's columns; just get
            // the new order onto disk.
            TableEvent::MoveColumn(..) => self.request_layout_save(cx),
            _ => {}
        }
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        crate::catalog::browse(&self.state.library, cx);
    }

    /// The shown columns in display order, each with its live width, for
    /// the layout dump and for duplicates.
    fn column_specs(&self, cx: &App) -> Vec<ColumnSpec> {
        let delegate = self.table.read(cx).delegate();
        delegate
            .columns
            .iter()
            .map(|column| ColumnSpec {
                key: column.key.to_string(),
                width: f32::from(column.width),
                label: delegate.labels.get(column.key.as_ref()).cloned(),
            })
            .collect()
    }

    /// The panel's live config, for the layout dump and for duplicates.
    fn config(&self, cx: &App) -> LibraryConfig {
        let sort = self.table.read(cx).delegate().sort.clone();
        LibraryConfig {
            chrome: self.chrome.clone(),
            query: self.query.clone(),
            search: self.show_search,
            query_source: self.query_source,
            row_height: Some(self.row_height),
            head_height: Some(self.head_height),
            row_spacing: self.row_spacing,
            head_text: self.head_text,
            // The legacy density folds in on load and never writes back.
            density: None,
            headers: self.headers,
            group_by: self.group_by,
            column_layout: self.column_specs(cx),
            sort_key: sort.as_ref().map(|(key, _)| key.to_string()),
            sort_desc: sort.is_some_and(|(_, desc)| desc),
            scroll_row: self.scroll_row(cx),
            follow_playing: self.follow_playing,
            resume_playing: self.resume_playing,
            smooth_follow: self.smooth_follow,
            art_rounding: self.art_rounding,
            art_side: self.art_side,
            art_margin: self.art_margin,
            header_gap_above: self.header_gap_above,
            header_gap_below: self.header_gap_below,
            header_art: self.header_art,
            portrait_circle: self.portrait_circle,
            genre_face: self.genre_face,
            header_flush: self.header_flush,
            header_compact: self.header_compact.clone(),
            header_lines: self.header_lines.clone(),
            // The legacy toggles fold in on load and never write back.
            header_year: None,
            header_details: None,
            compact_plays: self.compact_plays,
            stripes: self.stripes,
            row_borders: self.row_borders,
            column_headers: self.column_headers,
            sort_on_click: self.sort_on_click,
        }
    }

    /// The view row at the top of the viewport, read off the table's
    /// scroll handle. The uniform list never reports child bounds to its
    /// base handle, so the row comes from the pixel offset over the row
    /// height, the slider's value scaled by the app font, the same
    /// height every row renders at (the handle's own `last_item_size.item`
    /// is the viewport, not a row). A restore still pending (the panel
    /// never painted) reports its target, so an unshown panel round-trips
    /// its position instead of dropping to zero.
    fn scroll_row(&self, cx: &App) -> usize {
        if let Some(row) = self.restore_scroll {
            return row;
        }
        let table = self.table.read(cx);
        if let Some(ix) = table.vertical_scroll_handle.deferred_item_index() {
            return ix;
        }
        let offset = -table.vertical_scroll_handle.base_handle().offset().y;
        if offset <= px(0.) {
            return 0;
        }
        // The rendered rows scale by the app font times this panel's own
        // override. A dump save runs outside the panel's render, so the
        // render-time thread-local scale isn't in scope; read the override
        // off our own theme instead so the offset-to-row math still matches
        // the rows on screen.
        let panel_scale = self
            .chrome
            .theme
            .font_scale
            .map(|s| s.clamp(palette::PANEL_FONT_SCALE_MIN, palette::PANEL_FONT_SCALE_MAX))
            .unwrap_or(1.0);
        let scale = palette::font_scale() * panel_scale;
        if scale <= 0. {
            return 0;
        }
        // Rows are no longer uniform (header lines size to their content),
        // so the offset iterates the view's own heights. Layout dumps only,
        // never a paint, so the loop is fine.
        let delegate = table.delegate();
        let lines = delegate.head_lines.len().max(1);
        let stride = px(self.row_height + self.row_spacing) * scale;
        let mut y = px(0.);
        for (ix, row) in delegate.view.iter().enumerate() {
            y += match *row {
                Row::Head(_, line) => {
                    let mut h = self.head_height;
                    if line == 0 {
                        h += self.header_gap_above;
                    }
                    if line as usize + 1 >= lines {
                        h += self.header_gap_below;
                    }
                    px(h) * scale
                }
                _ => stride,
            };
            if y > offset {
                return ix;
            }
        }
        delegate.view.len().saturating_sub(1)
    }

    /// Show or hide a registry column, keeping the rest in place. A shown
    /// column appends at the end in its default width; hiding drops it.
    /// The table re-reads the delegate's columns and the view stays put.
    fn toggle_column(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let Some(def) = column_def(key) else { return };
        let mut sort_cleared = false;
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            if let Some(ix) = delegate.columns.iter().position(|c| c.key.as_ref() == key) {
                // Never let the last column go: an empty table has no
                // header to bring one back from.
                if delegate.columns.len() > 1 {
                    delegate.columns.remove(ix);
                    // A hidden sort column leaves no header to clear the
                    // sort; drop back to the canonical order instead.
                    if delegate
                        .sort
                        .as_ref()
                        .is_some_and(|(k, _)| k.as_ref() == key)
                    {
                        delegate.sort = None;
                        sort_cleared = true;
                    }
                }
            } else {
                // A column brought back by hand keeps whatever header it
                // was renamed to, the same way a restored one does.
                let label: SharedString = match delegate.labels.get(def.key) {
                    Some(label) => label.clone().into(),
                    None => def.label.into(),
                };
                let column = Column::new(def.key, label).width(px(def.default_width));
                // Same gate the restored layout builds under, or a column
                // would sort while it was added by hand this session and stop
                // sorting on the next launch.
                let column = if columns::sortable(def.key) {
                    column.sort(ColumnSort::Default)
                } else {
                    column
                };
                delegate.columns.push(if def.right {
                    column.text_right()
                } else {
                    column
                });
            }
            table.refresh(cx);
        });
        if sort_cleared {
            self.refresh_view(cx);
        }
        self.columns_shown = self.shown_columns(cx);
        // Turning the Similar column on is the first thing that asks for a
        // score, and nothing else would ask until the track changed.
        self.refresh_similarity(cx);
        self.refresh_title_bar(cx);
        self.request_layout_save(cx);
    }

    /// Rename a column's header, or drop the rename with None so the
    /// registry's label comes back. An empty name is a name: it's how a
    /// header is asked to draw blank, and it persists as one.
    ///
    /// Called from the header menu and from the rename window, both of
    /// which run outside the table's own update, so this takes the table
    /// the way [`Self::toggle_column`] does.
    fn set_column_label(&mut self, key: String, label: Option<String>, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            match label.clone() {
                Some(label) => delegate.labels.insert(key.clone(), label),
                None => delegate.labels.remove(&key),
            };
            // A hidden column has no built column to write to; its name
            // still lands on the map above and shows when it comes back.
            if let Some(column) = delegate.columns.iter_mut().find(|c| c.key.as_ref() == key) {
                column.name = match label.clone() {
                    Some(label) => label.into(),
                    None => column_def(&key)
                        .map_or_else(SharedString::default, |def| SharedString::from(def.label)),
                };
            }
            table.refresh(cx);
        });
        self.request_layout_save(cx);
    }

    /// The current name of a column: what the user typed over it, or the
    /// registry's label.
    fn column_label(&self, key: &str, cx: &App) -> Option<String> {
        self.table.read(cx).delegate().labels.get(key).cloned()
    }

    /// Open the rename window for one header, or focus the open one. It
    /// holds the panel weakly, like every other dialog over a panel, so
    /// closing the panel under it leaves a window that renames nothing.
    fn open_column_rename(&mut self, key: String, cx: &mut Context<Self>) {
        if let Some((open_key, handle)) = self.column_rename.take() {
            if open_key == key {
                if handle
                    .update(cx, |_, window, _| window.activate_window())
                    .is_ok()
                {
                    self.column_rename = Some((open_key, handle));
                    return;
                }
            } else {
                // A window over another header would keep writing to that
                // one; close it rather than juggle two.
                handle
                    .update(cx, |_, window, _| window.remove_window())
                    .ok();
            }
        }
        let Some(def) = column_def(&key) else { return };
        let current = self.column_label(&key, cx).unwrap_or_default();
        let title = SharedString::from(format!("rox - rename {}", def.label));
        let bounds = gpui::Bounds::centered(None, gpui::size(px(380.), px(205.)), cx);
        let state = self.state.clone();
        let panel = cx.weak_entity();
        let handle = panel::open_child_window(cx, title, bounds, None, {
            let key = key.clone();
            move |window, cx| {
                cx.new(|cx| {
                    ColumnRenameWindow::new(panel, state, key, def.label, current, window, cx)
                })
            }
        });
        self.column_rename = Some((key, handle));
    }

    /// Switch the header's plain click between sorting and doing nothing.
    /// While it's on, the widget's own sorting is off (the delegate runs
    /// the cycle and draws the arrow) and the column drag wants Alt, which
    /// [`Self::set_alt`] tracks; while it's off, both go back to stock.
    fn set_sort_on_click(&mut self, on: bool, cx: &mut Context<Self>) {
        self.sort_on_click = on;
        self.table.update(cx, |table, cx| {
            table.sortable = !on;
            // Off, the drag is always live again. On, it waits for Alt,
            // which nothing is holding at the moment the toggle flips.
            table.col_movable = !on;
            table.delegate_mut().sort_on_click = on;
            // The widget keeps its own copy of each column, and the sorts
            // taken while it was switched off never reached it. Re-reading
            // the delegate's columns puts its arrows back on the column
            // the list is actually sorted by.
            table.refresh(cx);
        });
        cx.notify();
        self.request_layout_save(cx);
    }

    /// Follow the Alt key while click-to-sort is on: the plain click is
    /// the sort, so the column drag only arms while Alt is held. gpui
    /// can't gate `on_drag` on a modifier, so the header is built with or
    /// without it and the modifier decides which.
    fn set_alt(&mut self, alt: bool, cx: &mut Context<Self>) {
        if !self.sort_on_click {
            return;
        }
        self.table.update(cx, |table, cx| {
            if table.col_movable == alt {
                return;
            }
            table.col_movable = alt;
            cx.notify();
        });
    }

    /// The keys of the currently shown columns, for the settings checklist.
    /// The dropdown reads the `columns_shown` copy instead, so it never
    /// touches the table while the row context menu builds mid-update.
    fn shown_columns(&self, cx: &App) -> HashSet<String> {
        self.table
            .read(cx)
            .delegate()
            .columns
            .iter()
            .map(|c| c.key.to_string())
            .collect()
    }

    /// The customize window's column picker: one checkable row per registry
    /// column, ticked while shown. Multi-select, so it stacks a checklist
    /// instead of the exclusive segmented control; the reset goes in the
    /// block's header.
    fn column_checklist(&self, cx: &mut Context<Self>) -> Div {
        let shown = self.shown_columns(cx);
        let mut list = div().flex().flex_col().gap(tokens::SPACE_XS);
        for def in columns::offered() {
            let key = def.key;
            let on = shown.contains(key);
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .py(px(1.))
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| this.toggle_column(key, cx)),
                    )
                    .child(settings_ui::checkbox(on))
                    .child(
                        div()
                            .text_color(if on {
                                palette::text()
                            } else {
                                palette::text_muted()
                            })
                            .child(def.label),
                    ),
            );
        }
        list
    }

    /// Restore the registry's default visible set and order.
    fn reset_columns(&mut self, cx: &mut Context<Self>) {
        let sort = self.table.read(cx).delegate().sort.clone();
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            // The renames survive: this row resets what shows and in what
            // order, and each renamed header has its own reset in the
            // header menu.
            delegate.columns = track_columns(&[], &sort, &delegate.labels);
            table.refresh(cx);
        });
        self.columns_shown = self.shown_columns(cx);
        self.refresh_title_bar(cx);
        self.request_layout_save(cx);
    }

    /// Nudge the dock to persist the layout after a column change it never
    /// sees on its own: a resize, reorder, or toggle. The panel's own events
    /// don't reach the dock, but its host tab panel's do, so bounce a
    /// LayoutChanged through it and the workspace's debounced save picks the
    /// new columns up. Without this the columns only reach disk on a clean
    /// close or the next unrelated dock change, so a relaunch can lose them.
    fn request_layout_save(&self, cx: &mut Context<Self>) {
        if let Some(tabs) = self.tab_panel.as_ref().and_then(|w| w.upgrade()) {
            tabs.update(cx, |_, cx| cx.emit(PanelEvent::LayoutChanged));
        }
    }

    /// While docked, the panel's controls are in the tab panel's title bar,
    /// which only repaints when the tab panel itself is notified. Call this
    /// after any change the title bar shows: query, focus, status, error.
    fn refresh_title_bar(&self, cx: &mut App) {
        if let Some(tabs) = self.tab_panel.as_ref().and_then(|tabs| tabs.upgrade()) {
            tabs.update(cx, |_, cx| cx.notify());
        }
    }

    /// Queue the double-clicked track as the start of a natural progression
    /// through the view: the tracks before it seed behind the cursor so Prev
    /// steps back, the ones after take Next on through the library, and the
    /// clicked track plays. This is the playing context, not the queue, so it
    /// never shows in the queue panel; the window is bounded so a huge view
    /// doesn't materialize whole, with a share of the budget kept for history.
    /// Headers pass under the cap, so it counts tracks.
    fn play_from(&mut self, ix: usize, cx: &mut Context<Self>) {
        // With shuffle on, draw from the whole view, not just the rows after
        // the clicked one; the engine pins the head, so the clicked track
        // still plays first while everything else shuffles in behind it.
        if self.state.player.read(cx).shuffle() {
            self.play_shuffled_from(ix, cx);
            return;
        }
        let window = {
            let view = self.table.read(cx).delegate().view.clone();
            play_window(&view, ix, QUEUE_CAP)
        };
        let Some((rows, start)) = window else { return };
        self.play_rows_at(rows, start, false, cx);
    }

    /// Resolve view rows to paths and play them as the up-next queue, from
    /// the first. The explicit-selection play: an album or a hand-picked set
    /// shows in the queue panel, unlike a library run, which stays context.
    fn play_rows(&mut self, rows: Vec<usize>, cx: &mut Context<Self>) {
        self.play_rows_at(rows, 0, true, cx);
    }

    /// Resolve view rows to paths and queue them on the shared player with
    /// the cursor at `start`. `explicit` marks them the up-next queue so the
    /// queue panel lists them (an album, a selection); a context run (a
    /// library run, a shuffle) passes false and starts at `start`.
    fn play_rows_at(
        &mut self,
        rows: Vec<usize>,
        start: usize,
        explicit: bool,
        cx: &mut Context<Self>,
    ) {
        let (result, scope) = {
            let view = self.table.read(cx).delegate().view.clone();
            let library = self.state.library.read(cx);
            let Some(projection) = library.projection() else {
                return;
            };
            let ids: Vec<i64> = rows
                .into_iter()
                .filter_map(|ix| match view.get(ix) {
                    Some(&Row::Track(row)) => Some(projection.db_id[row as usize]),
                    _ => None,
                })
                .collect();
            // The whole view, not the window that got queued. A big view
            // plays in a bounded slice (see `play_from`), so the rows below
            // the slice are exactly what continuation runs on into
            // (ADR 17); handing over only what was queued would leave it
            // nothing to resume.
            let order: Vec<i64> = view
                .iter()
                .filter_map(|row| match row {
                    &Row::Track(row) => Some(projection.db_id[row as usize]),
                    _ => None,
                })
                .collect();
            (
                library.keys_for(&ids),
                continuation::Scope::View(order.into()),
            )
        };
        match result {
            Ok(keys) => self.state.player.update(cx, |player, cx| {
                if explicit {
                    player.play_explicit(keys, cx);
                } else {
                    player.play_at(keys, start, cx);
                }
                // After the play, never before: starting a session clears
                // the scope back to the library at large.
                player.set_scope(scope);
            }),
            Err(e) => {
                self.error = Some(format!("library: {e}").into());
                cx.notify();
                self.refresh_title_bar(cx);
            }
        }
    }

    /// Play something that sounds like the clicked track, drawn library-wide
    /// off the acoustic vectors.
    ///
    /// The clicked track itself doesn't play. The ask is for more like it,
    /// and a double click already plays the row; an earlier
    /// cut played it and reordered the view behind it, which meant the entry
    /// did nothing you could hear until the track after this one.
    fn play_similar(&mut self, row_ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.table.read(cx).delegate().track_at(row_ix) else {
            return;
        };
        let Some(&id) = self
            .state
            .library
            .read(cx)
            .projection()
            .and_then(|projection| projection.db_id.get(row as usize))
        else {
            return;
        };
        let library = self.state.library.clone();
        self.state
            .player
            .update(cx, |player, cx| player.play_similar_to(id, &library, cx));
    }

    /// Turn shuffle on, then queue `rows` from the front. The engine pins the
    /// head when shuffle engages, so the first row plays first and the rest
    /// draw in a random order. Rows past the cap drop.
    fn play_shuffled(&mut self, mut rows: Vec<usize>, cx: &mut Context<Self>) {
        rows.truncate(QUEUE_CAP);
        self.state
            .player
            .update(cx, |player, _| player.set_shuffle(true));
        self.play_rows_at(rows, 0, false, cx);
    }

    /// Play the view shuffled with `ix` first: the clicked row heads the
    /// queue so the pinned head plays before the shuffled rest. "Play Shuffled"
    /// on a single row and a shuffle-on double click both come through here.
    ///
    /// The draw is the clicked row plus the view's leading rows up to the
    /// cap, which is what the whole-view build came to anyway once
    /// [`Self::play_shuffled`] truncated it; taking only that many out of
    /// the view keeps a click on a million-row list from listing the whole
    /// thing to throw all but a thousand of it away.
    fn play_shuffled_from(&mut self, ix: usize, cx: &mut Context<Self>) {
        let rows = {
            let delegate = self.table.read(cx).delegate();
            let mut tracks = Vec::with_capacity(QUEUE_CAP.min(delegate.view.len()));
            // A press on a header row plays its view from the front, the
            // way it always did; only a track row heads the queue.
            if delegate.track_at(ix).is_some() {
                tracks.push(ix);
            }
            tracks.extend(
                (0..delegate.view.len())
                    .filter(|&i| i != ix && delegate.track_at(i).is_some())
                    .take(QUEUE_CAP - tracks.len()),
            );
            tracks
        };
        self.play_shuffled(rows, cx);
    }

    fn on_search_event(
        &mut self,
        _search: &Entity<SearchBox>,
        event: &SearchEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SearchEvent::Changed => self.on_query_box_changed(cx),
            // The input's focus ring renders in the title bar while the
            // panel shares a group, and that row only repaints when the
            // tab panel is notified.
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

    fn search_box(&self, _window: &Window, cx: &mut Context<Self>) -> Div {
        self.search.update(cx, |search, cx| search.element(cx))
    }

    /// The popped-out window has no title bar to host the controls, so it
    /// keeps them as a toolbar row above the list. The catalog status shows
    /// in the workspace menubar; only a panel-local error shows here.
    fn toolbar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex_none()
            .h(px(36.))
            .px(tokens::SPACE_SM)
            .gap(tokens::SPACE_SM)
            .flex()
            .flex_row()
            .items_center()
            .bg(palette::bg_toolbar())
            .border_b_1()
            .border_color(palette::border())
            .when(self.show_search, |d| {
                d.child(self.search_box(window, cx).flex_1())
            })
            .when_some(self.error.clone(), |d, error| {
                d.child(
                    div()
                        .flex_none()
                        .text_color(palette::text_muted())
                        .child(error),
                )
            })
    }

    fn track_list(&self) -> impl IntoElement {
        Table::new(&self.table)
            .stripe(self.stripes)
            .row_borders(self.row_borders)
            .header_visible(self.column_headers)
            .bordered(false)
            // The rows draw their own selection wash, so the widget's
            // overlay stays off; it would swap the clicked row's bottom
            // hairline for a ring the row clip eats (vendor patch).
            .row_selection_style(false)
            .row_spacing(px(self.row_spacing))
            // A custom size is the row height itself (vendor patch); the
            // widget scales it by the window rem like the stock sizes.
            .with_size(Size::Size(px(self.row_height)))
    }

    /// Set the track rows' height and rebuild the view: the header
    /// blocks' row spans follow the ratio of the two heights. Persisted on
    /// the next layout dump.
    fn set_row_height(&mut self, height: f32, cx: &mut Context<Self>) {
        if self.row_height == height {
            return;
        }
        self.row_height = height;
        self.table
            .update(cx, |table, _| table.delegate_mut().row_height = height);
        self.refresh_view(cx);
        cx.notify();
        self.refresh_title_bar(cx);
    }

    /// Set the open gap under each track row and rebuild the view: the
    /// gap is part of the row stride, which the header blocks' row spans
    /// run on, the heights' route.
    fn set_row_spacing(&mut self, spacing: f32, cx: &mut Context<Self>) {
        if self.row_spacing == spacing {
            return;
        }
        self.row_spacing = spacing;
        self.table
            .update(cx, |table, _| table.delegate_mut().row_spacing = spacing);
        self.refresh_view(cx);
        cx.notify();
    }

    /// Set the header lines' text size. Pure paint: the block rows size
    /// to the line height, not the text.
    fn set_head_text(&mut self, size: f32, cx: &mut Context<Self>) {
        if self.head_text == size {
            return;
        }
        self.head_text = size;
        self.table
            .update(cx, |table, _| table.delegate_mut().head_text = size);
        cx.notify();
    }

    /// Set one header line's height. The block rows resize to it through
    /// the delegate's height hook; the view's rows don't move, so no
    /// rebuild and the selection stays put.
    fn set_head_height(&mut self, height: f32, cx: &mut Context<Self>) {
        if self.head_height == height {
            return;
        }
        self.head_height = height;
        self.table
            .update(cx, |table, _| table.delegate_mut().head_height = height);
        cx.notify();
    }

    /// Set the open space over each header block, the line height's
    /// route: the block's first row grows by it. Persisted on the next
    /// layout dump.
    fn set_header_gap_above(&mut self, gap: f32, cx: &mut Context<Self>) {
        if self.header_gap_above == gap {
            return;
        }
        self.header_gap_above = gap;
        self.table
            .update(cx, |table, _| table.delegate_mut().header_gap_above = gap);
        cx.notify();
    }

    /// The same under the block, on its last row.
    fn set_header_gap_below(&mut self, gap: f32, cx: &mut Context<Self>) {
        if self.header_gap_below == gap {
            return;
        }
        self.header_gap_below = gap;
        self.table
            .update(cx, |table, _| table.delegate_mut().header_gap_below = gap);
        cx.notify();
    }

    /// Set which side the header blocks' cover tile sits on and repaint;
    /// persisted on the next layout dump.
    fn set_art_side(&mut self, side: ArtSide, cx: &mut Context<Self>) {
        if self.art_side == side {
            return;
        }
        self.art_side = side;
        self.table.update(cx, |table, cx| {
            table.delegate_mut().art_side = side;
            cx.notify();
        });
        cx.notify();
    }

    /// Set the cover tile's inset inside the block and repaint; the tile
    /// shrinks in place, so no view rebuild.
    fn set_art_margin(&mut self, margin: f32, cx: &mut Context<Self>) {
        if self.art_margin == margin {
            return;
        }
        self.art_margin = margin;
        self.table.update(cx, |table, cx| {
            table.delegate_mut().art_margin = margin;
            cx.notify();
        });
        cx.notify();
    }

    /// Set the header style and rebuild the view; persisted on the next
    /// layout dump. The mode picks which composed lines render, so the
    /// delegate's copy swaps with it.
    fn set_headers(&mut self, headers: Headers, cx: &mut Context<Self>) {
        if self.headers == headers {
            return;
        }
        self.headers = headers;
        let lines = effective_head_lines(headers, &self.header_compact, &self.header_lines);
        self.table.update(cx, |table, _| {
            let delegate = table.delegate_mut();
            delegate.headers = headers;
            delegate.head_lines = lines;
        });
        self.refresh_view(cx);
        cx.notify();
        self.refresh_title_bar(cx);
    }

    /// Store the rows editor's wells back into the fixed line slots and
    /// rebuild: the block's row count follows the non-empty lines. The
    /// editor sends every open well, empties included, so the open count
    /// follows its adds and removes; slots past it clear. Persisted on
    /// the next layout dump.
    fn set_head_lines(&mut self, rows: Vec<Vec<HeadPiece>>, cx: &mut Context<Self>) {
        self.header_lines_shown = rows.len().clamp(1, HEAD_LINE_SLOTS);
        for slot in 0..HEAD_LINE_SLOTS {
            if let Some(line) = self.header_lines.get_mut(slot) {
                *line = rows.get(slot).cloned().unwrap_or_default();
            }
        }
        self.sync_head_lines(cx);
    }

    /// Store the edited compact row and rebuild, the expanded slots' route.
    fn set_head_compact(&mut self, items: Vec<HeadPiece>, cx: &mut Context<Self>) {
        self.header_compact = items;
        self.sync_head_lines(cx);
    }

    /// Copy the current mode's composed lines into the delegate and
    /// rebuild the view; a line count change respans every header block.
    fn sync_head_lines(&mut self, cx: &mut Context<Self>) {
        let lines = effective_head_lines(self.headers, &self.header_compact, &self.header_lines);
        self.table
            .update(cx, |table, _| table.delegate_mut().head_lines = lines);
        self.refresh_view(cx);
        cx.notify();
    }

    /// Flip the plays column's compact face and repaint the rows;
    /// persisted on the next layout dump like the other view knobs.
    fn set_compact_plays(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.compact_plays == on {
            return;
        }
        self.compact_plays = on;
        self.table.update(cx, |table, cx| {
            table.delegate_mut().compact_plays = on;
            cx.notify();
        });
        cx.notify();
    }

    /// Set what the headers group on and rebuild the view; persisted on
    /// the next layout dump like the header style.
    fn set_group_by(&mut self, group_by: GroupBy, cx: &mut Context<Self>) {
        if self.group_by == group_by {
            return;
        }
        self.group_by = group_by;
        self.table
            .update(cx, |table, _| table.delegate_mut().group_by = group_by);
        self.refresh_view(cx);
        cx.notify();
        self.refresh_title_bar(cx);
    }

    /// The Layout page: what the group headers are and how their lines
    /// compose. The look knobs (heights, gaps, art) stay on Appearance,
    /// the column checklist on View.
    fn layout_page(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let header_mode = self.headers;
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_row(
                rox_i18n::t!("library-headers"),
                Some(rox_i18n::t!("library-headers.description")),
                panel::choices_shared(
                    &[
                        (rox_i18n::t!("headers-off"), Headers::Off),
                        (rox_i18n::t!("headers-compact"), Headers::Compact),
                        (rox_i18n::t!("headers-expanded"), Headers::Expanded),
                    ],
                    header_mode,
                    |this: &mut Self, headers, cx| this.set_headers(headers, cx),
                    cx,
                ),
            ))
            .when(header_mode != Headers::Off, |d| {
                d.child(panel::setting_row(
                    rox_i18n::t!("library-group-by"),
                    Some(rox_i18n::t!("library-group-by.description")),
                    panel::choices_shared(
                        &[
                            (rox_i18n::t!("head-piece-album"), GroupBy::Album),
                            (rox_i18n::t!("head-piece-artist"), GroupBy::Artist),
                            (rox_i18n::t!("head-piece-genre"), GroupBy::Genre),
                            (rox_i18n::t!("head-piece-year"), GroupBy::Year),
                        ],
                        self.group_by,
                        |this: &mut Self, group_by, cx| this.set_group_by(group_by, cx),
                        cx,
                    ),
                ))
            })
            .when(header_mode == Headers::Compact, |d| {
                d.child(panel::setting_block(
                    rox_i18n::t!("library-header-row"),
                    Some(rox_i18n::t!("library-header-row.description")),
                    None,
                    panel::arrange_editor(
                        "library-head-compact",
                        group_head::PIECES,
                        &self.header_compact,
                        |this: &mut Self, items, cx| this.set_head_compact(items, cx),
                        cx,
                    ),
                ))
            })
            .when(header_mode == Headers::Expanded, |d| {
                // One well per line, top to bottom; a line left empty drops
                // out of the block, so two make the classic pair and three
                // the tall foobar-style block.
                let open = self.header_lines_shown.clamp(1, HEAD_LINE_SLOTS);
                d.child(panel::setting_block(
                    rox_i18n::t!("library-header-lines"),
                    Some(rox_i18n::t!("library-header-lines.description")),
                    None,
                    panel::arrange_rows_editor(
                        "library-head-lines",
                        group_head::PIECES,
                        &self.header_lines[..open],
                        Some(HEAD_LINE_SLOTS),
                        |this: &mut Self, rows, cx| this.set_head_lines(rows, cx),
                        cx,
                    ),
                ))
            })
            .into_any_element()
    }

    fn empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("library-empty")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(tokens::SPACE_SM)
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.browse(cx)))
            .child(div().text_lg().child(rox_i18n::t!("library-empty-title")))
            .child(
                div()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("library-empty-note")),
            )
    }
}

impl panel::PanelSettings for LibraryPanel {
    fn state(&self) -> AppState {
        self.state.clone()
    }

    fn chrome(&self) -> &PanelChrome {
        &self.chrome
    }

    fn chrome_mut(&mut self) -> &mut PanelChrome {
        &mut self.chrome
    }

    fn set_custom_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.chrome.title = title;
        panel::refresh_tab_panel(&self.tab_panel, cx);
        cx.notify();
    }

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[("Layout", icons::ALIGN_LEFT), ("View", icons::ROWS_3)]
    }

    fn behavior(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        Some(
            div()
                .flex()
                .flex_col()
                .gap(settings_ui::SECTION_GAP)
                .child(crate::query::shared_query::search_section(
                    self.show_search,
                    |this: &mut Self, on, cx| {
                        this.show_search = on;
                        // The box keeps its text; the view snaps to the
                        // full catalog while hidden.
                        this.refresh_view(cx);
                        cx.notify();
                        this.refresh_title_bar(cx);
                    },
                    self.query_source,
                    |this: &mut Self, source, cx| this.pick_query_source(source, cx),
                    cx,
                ))
                .child(panel::tracking_section(
                    self.follow_playing,
                    rox_i18n::t!("library-follow-description"),
                    |this: &mut Self, on, cx| {
                        this.follow_playing = on;
                        // Catch up right away instead of waiting for
                        // the next track change.
                        if on {
                            this.follow_playing(cx);
                        }
                        cx.notify();
                    },
                    self.resume_playing,
                    rox_i18n::t!("library-resume-description"),
                    |this: &mut Self, on, cx| {
                        this.resume_playing = on;
                        cx.notify();
                    },
                    self.smooth_follow,
                    rox_i18n::t!("library-smooth-description"),
                    |this: &mut Self, on, cx| {
                        this.smooth_follow = on;
                        cx.notify();
                    },
                    cx,
                ))
                .child(panel::setting_row(
                    rox_i18n::t!("library-sort-on-click"),
                    Some(rox_i18n::t!("library-sort-on-click.description")),
                    panel::toggle(
                        self.sort_on_click,
                        |this: &mut Self, on, cx| this.set_sort_on_click(on, cx),
                        cx,
                    ),
                ))
                .into_any_element(),
        )
    }

    fn page(
        &mut self,
        page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        if page == "Layout" {
            return self.layout_page(cx);
        }
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_block(
                rox_i18n::t!("library-columns"),
                Some(rox_i18n::t!("library-columns.description")),
                Some(
                    settings_ui::small_button(
                        rox_i18n::t!("panel-reset"),
                        icons::REFRESH_CW,
                        false,
                        cx.listener(|this, _, _, cx| this.reset_columns(cx)),
                    )
                    .into_any_element(),
                ),
                self.column_checklist(cx),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("library-column-headers"),
                Some(rox_i18n::t!("library-column-headers.description")),
                panel::toggle(
                    self.column_headers,
                    |this: &mut Self, on, cx| {
                        this.column_headers = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("library-compact-plays"),
                Some(rox_i18n::t!("library-compact-plays.description")),
                panel::toggle(
                    self.compact_plays,
                    |this: &mut Self, on, cx| this.set_compact_plays(on, cx),
                    cx,
                ),
            ))
            .into_any_element()
    }

    /// The library's own appearance rows on the shared page: what shapes
    /// the rows and their group headers, from the heights and striping to
    /// the gaps and the cover tile. These are stored on the config because
    /// they shape the content, not the panel frame; the Layout page holds
    /// the headers' composition, the View page what shows (columns,
    /// search).
    fn appearance(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let rounding = self.art_rounding;
        let row_height = self.row_height;
        let row_spacing = self.row_spacing;
        let head_height = self.head_height;
        let head_text = self.head_text;
        let gap_above = self.header_gap_above;
        let gap_below = self.header_gap_below;
        let art_margin = self.art_margin;
        let header_mode = self.headers;
        let headers = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_row(
                rox_i18n::t!("library-line-height"),
                Some(rox_i18n::t!("library-line-height.description")),
                settings_ui::scalar(
                    &self.head_scrub,
                    &self.value_edit,
                    head_height,
                    settings_ui::span(ROW_HEIGHT_MIN, HEAD_HEIGHT_MAX, " px"),
                    Self::set_head_height,
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("library-text-size"),
                Some(rox_i18n::t!("library-text-size.description")),
                settings_ui::scalar(
                    &self.head_text_scrub,
                    &self.value_edit,
                    head_text,
                    settings_ui::span(HEAD_TEXT_MIN, HEAD_TEXT_MAX, " px"),
                    Self::set_head_text,
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("library-flush-background"),
                Some(rox_i18n::t!("library-flush-background.description")),
                panel::toggle(
                    self.header_flush,
                    |this: &mut Self, on, cx| {
                        this.header_flush = on;
                        this.table
                            .update(cx, |table, _| table.delegate_mut().header_flush = on);
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("library-gap-above"),
                Some(rox_i18n::t!("library-gap-above.description")),
                settings_ui::scalar(
                    &self.header_gap_above_scrub,
                    &self.value_edit,
                    gap_above,
                    settings_ui::span(0., HEAD_GAP_MAX, " px"),
                    Self::set_header_gap_above,
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("library-gap-below"),
                Some(rox_i18n::t!("library-gap-below.description")),
                settings_ui::scalar(
                    &self.header_gap_below_scrub,
                    &self.value_edit,
                    gap_below,
                    settings_ui::span(0., HEAD_GAP_MAX, " px"),
                    Self::set_header_gap_below,
                    cx,
                ),
            ));
        Some(
            div()
                .flex()
                .flex_col()
                .gap(settings_ui::SECTION_GAP)
                .child(settings_ui::section(
                    rox_i18n::t!("library-section-rows"),
                    None,
                    div()
                        .flex()
                        .flex_col()
                        .gap(tokens::SPACE_MD)
                        .child(panel::setting_row(
                            rox_i18n::t!("library-row-height"),
                            Some(rox_i18n::t!("library-row-height.description")),
                            settings_ui::scalar(
                                &self.row_scrub,
                                &self.value_edit,
                                row_height,
                                settings_ui::span(ROW_HEIGHT_MIN, ROW_HEIGHT_MAX, " px"),
                                Self::set_row_height,
                                cx,
                            ),
                        ))
                        .child(panel::setting_row(
                            rox_i18n::t!("library-row-spacing"),
                            Some(rox_i18n::t!("library-row-spacing.description")),
                            settings_ui::scalar(
                                &self.row_spacing_scrub,
                                &self.value_edit,
                                row_spacing,
                                settings_ui::span(0., ROW_SPACING_MAX, " px"),
                                Self::set_row_spacing,
                                cx,
                            ),
                        ))
                        .child(panel::setting_row(
                            rox_i18n::t!("library-stripes"),
                            Some(rox_i18n::t!("library-stripes.description")),
                            panel::toggle(
                                self.stripes,
                                |this: &mut Self, on, cx| {
                                    this.stripes = on;
                                    cx.notify();
                                },
                                cx,
                            ),
                        ))
                        .child(panel::setting_row(
                            rox_i18n::t!("library-row-borders"),
                            Some(rox_i18n::t!("library-row-borders.description")),
                            panel::toggle(
                                self.row_borders,
                                |this: &mut Self, on, cx| {
                                    this.row_borders = on;
                                    cx.notify();
                                },
                                cx,
                            ),
                        )),
                ))
                // The header look only matters while headers show; their
                // mode and composition are on the Layout page.
                .when(header_mode != Headers::Off, |d| {
                    d.child(settings_ui::section(
                        rox_i18n::t!("library-headers"),
                        None,
                        headers,
                    ))
                })
                // Every art knob in one always-shown place, whatever the
                // grouping: swapping the group-by shouldn't send you
                // hunting for the row that just appeared elsewhere.
                .child(settings_ui::section(
                    rox_i18n::t!("head-piece-art"),
                    None,
                    div()
                        .flex()
                        .flex_col()
                        .gap(tokens::SPACE_MD)
                        .child(panel::setting_row(
                            rox_i18n::t!("head-piece-art"),
                            Some(rox_i18n::t!("library-art-description")),
                            panel::toggle(
                                self.header_art,
                                |this: &mut Self, on, cx| {
                                    this.header_art = on;
                                    this.table.update(cx, |table, _| {
                                        table.delegate_mut().header_art = on
                                    });
                                    cx.notify();
                                },
                                cx,
                            ),
                        ))
                        .child(panel::setting_row(
                            rox_i18n::t!("library-art-rounding"),
                            Some(rox_i18n::t!("library-art-rounding.description")),
                            settings_ui::scalar(
                                &self.art_scrub,
                                &self.value_edit,
                                rounding,
                                settings_ui::span(0., ART_ROUNDING_MAX, " px"),
                                |this: &mut Self, value, cx| {
                                    this.art_rounding = value;
                                    // The delegate copies it for the tile render,
                                    // the heights' route.
                                    this.table.update(cx, |table, _| {
                                        table.delegate_mut().art_rounding = value
                                    });
                                    cx.notify();
                                },
                                cx,
                            ),
                        ))
                        .child(panel::setting_row(
                            rox_i18n::t!("library-art-position"),
                            Some(rox_i18n::t!("library-art-position.description")),
                            panel::choices_shared(
                                &[
                                    (rox_i18n::t!("side-left"), ArtSide::Left),
                                    (rox_i18n::t!("side-right"), ArtSide::Right),
                                ],
                                self.art_side,
                                |this: &mut Self, side, cx| this.set_art_side(side, cx),
                                cx,
                            ),
                        ))
                        .child(panel::setting_row(
                            rox_i18n::t!("library-art-margin"),
                            Some(rox_i18n::t!("library-art-margin.description")),
                            settings_ui::scalar(
                                &self.art_margin_scrub,
                                &self.value_edit,
                                art_margin,
                                settings_ui::span(0., ART_MARGIN_MAX, " px"),
                                Self::set_art_margin,
                                cx,
                            ),
                        ))
                        .child(panel::setting_row(
                            rox_i18n::t!("library-circular-portraits"),
                            Some(rox_i18n::t!("library-circular-portraits.description")),
                            panel::toggle(
                                self.portrait_circle,
                                |this: &mut Self, on, cx| {
                                    this.portrait_circle = on;
                                    this.table.update(cx, |table, _| {
                                        table.delegate_mut().portrait_circle = on
                                    });
                                    cx.notify();
                                },
                                cx,
                            ),
                        ))
                        .child(panel::setting_row(
                            rox_i18n::t!("library-genre-face"),
                            Some(rox_i18n::t!("library-genre-face.description")),
                            panel::choices_shared(
                                &[
                                    (rox_i18n::t!("genre-face-mosaic"), TileFace::Mosaic),
                                    (rox_i18n::t!("genre-face-tinted"), TileFace::Tinted),
                                    (rox_i18n::t!("genre-face-gradient"), TileFace::Gradient),
                                    (rox_i18n::t!("genre-face-color"), TileFace::Color),
                                ],
                                self.genre_face,
                                |this: &mut Self, face, cx| {
                                    this.genre_face = face;
                                    this.table.update(cx, |table, _| {
                                        table.delegate_mut().genre_face = face
                                    });
                                    cx.notify();
                                },
                                cx,
                            ),
                        )),
                ))
                .into_any_element(),
        )
    }
}

impl EventEmitter<PanelEvent> for LibraryPanel {}

impl Focusable for LibraryPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl QueryFilter for LibraryPanel {
    fn shared_query(&self) -> &Entity<crate::query::shared_query::SharedQuery> {
        &self.state.query
    }
    fn query_box(&self) -> &Entity<SearchBox> {
        &self.search
    }
    fn query_source(&self) -> QuerySource {
        self.query_source
    }
    fn set_query_source_value(&mut self, source: QuerySource) {
        self.query_source = source;
    }
    fn local_query(&self) -> String {
        self.query.clone()
    }
    fn set_local_query(&mut self, query: String) {
        self.query = query;
    }
    fn query_box_shown(&self) -> bool {
        self.show_search
    }
    fn set_query_box_shown(&mut self, shown: bool) {
        self.show_search = shown;
    }
    /// Every query change reaches the view through here, and the ones that
    /// matter for cost are keystrokes, so this is the debounced path.
    fn rebuild_query_view(&mut self, cx: &mut Context<Self>) {
        self.refresh_view_debounced(cx);
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

impl Panel for LibraryPanel {
    fn panel_name(&self) -> &'static str {
        "library"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.chrome.title.as_deref(),
            rox_i18n::t!("panel-title-library"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.chrome.title.clone().map(SharedString::from)
    }

    /// The panel's controls share the title bar row instead of stacking a
    /// second toolbar row under it. Kept compact: the title row is 30px.
    fn title_suffix(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        if !self.show_search && self.error.is_none() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .flex_none()
                .gap(tokens::SPACE_SM)
                .when(self.show_search, |d| {
                    d.child(self.search_box(window, cx).w(px(180.)))
                })
                .when_some(self.error.clone(), |d, error| {
                    d.child(
                        div()
                            .max_w(px(240.))
                            .truncate()
                            .text_color(palette::text_muted())
                            .child(error),
                    )
                }),
        )
    }

    fn locked(&self, _cx: &App) -> bool {
        self.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    /// The table serves row context menus over the whole body, so the tab
    /// panel's body right-click stays out; the panel dropdown is on the
    /// tab and the toolbar.
    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_min_size(
            &self.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_max_size(&self.chrome, self.min_size(cx))
    }

    /// The layout dump stores the panel's config; the builder registered
    /// in `workspace::register_panels` reads it back.
    fn dump(&self, cx: &App) -> PanelState {
        let config = self.config(cx);
        let mut state = PanelState::new(self);
        state.info =
            PanelInfo::panel(serde_json::to_value(config).unwrap_or(serde_json::Value::Null));
        state
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel.clone());
        self._tabs_changed = tab_panel
            .upgrade()
            .map(|tabs| cx.observe(&tabs, |_, _, cx| cx.notify()));
        self.state
            .tab_hosts
            .update(cx, |hosts, _| hosts.report(tab_panel));
    }

    fn on_removed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.tab_panel = None;
        self._tabs_changed = None;
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        // Jump and Follow go at the top; the view knobs group under a
        // Display flyout below so the menu stays short. Every entry
        // dismisses the menu on click, and the next open rebuilds with the
        // change reflected. The customize window still holds the same
        // knobs with real controls.
        let weak = cx.entity().downgrade();
        let weak_f = cx.entity().downgrade();
        let follow = self.follow_playing;
        // Checks on the right so these two keep their icons; the default
        // left side would swap the check in for the icon.
        let menu = menu
            .check_side(Side::Right)
            .item(
                PopupMenuItem::new(rox_i18n::t!("library-jump-to-playing"))
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

        // Display section: the view knobs, one flyout per setting so the
        // menu stays short. The flyouts build eagerly off the panel's
        // copies, never the table: this menu also builds inside the row
        // context menu, mid-table-update.
        let menu = menu.separator().label(rox_i18n::t!("library-menu-display"));

        // Columns: the same toggles as the header dropdown and the settings
        // checklist, one row per registry column ticked while shown, read off
        // the panel's copy.
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            for def in columns::offered() {
                let key = def.key;
                submenu = submenu.item(panel::check_row(
                    def.label,
                    None,
                    move |this: &Self| this.columns_shown.contains(key),
                    move |this, cx| this.toggle_column(key, cx),
                    &panel,
                ));
            }
            submenu
        });
        let menu = menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("library-columns"),
            submenu,
        ));

        // The column header row's toggle, placed beside the columns it heads.
        let weak_h = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new(rox_i18n::t!("library-column-headers"))
                .checked(self.column_headers)
                .on_click(move |_, _, cx| {
                    if let Some(this) = weak_h.upgrade() {
                        this.update(cx, |this, cx| {
                            this.column_headers = !this.column_headers;
                            cx.notify();
                        });
                    }
                }),
        );

        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            let mut submenu = submenu.check_side(Side::Right);
            for (headers, name) in [
                (Headers::Off, rox_i18n::t!("headers-off")),
                (Headers::Compact, rox_i18n::t!("headers-compact")),
                (Headers::Expanded, rox_i18n::t!("headers-expanded")),
            ] {
                submenu = submenu.item(panel::check_row(
                    name,
                    None,
                    move |this: &Self| this.headers == headers,
                    move |this, cx| this.set_headers(headers, cx),
                    &panel,
                ));
            }
            submenu
        });
        let mut menu = menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("library-headers"),
            submenu,
        ));

        if self.headers != Headers::Off {
            let panel = cx.entity();
            let submenu = PopupMenu::build(window, cx, move |submenu, _, cx| {
                panel::follow_panel(&panel, cx);
                let mut submenu = submenu.check_side(Side::Right);
                for (group_by, name) in [
                    (GroupBy::Album, rox_i18n::t!("head-piece-album")),
                    (GroupBy::Artist, rox_i18n::t!("head-piece-artist")),
                    (GroupBy::Genre, rox_i18n::t!("head-piece-genre")),
                    (GroupBy::Year, rox_i18n::t!("head-piece-year")),
                ] {
                    submenu = submenu.item(panel::check_row(
                        name,
                        None,
                        move |this: &Self| this.group_by == group_by,
                        move |this, cx| this.set_group_by(group_by, cx),
                        &panel,
                    ));
                }
                submenu
            });
            menu = menu.item(PopupMenuItem::submenu(
                rox_i18n::t!("library-group-by"),
                submenu,
            ));
        }

        // Follow the shared search query, or filter by this panel's own box.
        let menu = crate::query::shared_query::search_flyout(
            menu,
            |this: &Self| this.query_source,
            |this: &Self| this.show_search,
            &cx.entity(),
            |this: &mut Self, source, cx| this.pick_query_source(source, cx),
            |this: &mut Self, on, cx| {
                this.show_search = on;
                // The box keeps its text; the view snaps to the full catalog
                // while hidden.
                this.refresh_view(cx);
                cx.notify();
                this.refresh_title_bar(cx);
            },
            window,
            cx,
        );

        // Panel section: operations on the panel itself, not its contents.
        // Duplicate copies this view's config, the query included, over the
        // same catalog and player.
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
                    (panel.state.clone(), panel.config(cx))
                };
                LibraryPanel::new(state, config, window, cx)
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

impl Render for LibraryPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl LibraryPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // A pending box reset (a source toggle or a shared-query change)
        // is applied here, where a window exists to set the input's text.
        if self.resync_box {
            self.resync_box = false;
            self.sync_query_box(window, cx);
        }
        // An extractor switch and a finished analysis pass both arrive as a
        // repaint and nothing else, so this is where the Similar column
        // picks them up.
        self.watch_similarity(cx);
        // The follow glide eases toward the playing row, stepped here in
        // render (the cover panel's fade idiom), one frame at a time until
        // it arrives.
        let dt = self.glide_tick.elapsed().as_secs_f32().min(0.05);
        self.glide_tick = Instant::now();
        if let Some(row) = self.glide_to {
            let (handle, target, in_view) = {
                let table = self.table.read(cx);
                // Header rows size to their content, so a uniform-stride
                // estimate puts the row off center or off screen. The
                // table's cached per-row heights give the row's real
                // offset, so the glide centers it exactly.
                let target = table.row_bounds(row).and_then(|(y, h)| {
                    panel::glide_target_at(
                        table.vertical_scroll_handle.base_handle(),
                        gpui::Axis::Vertical,
                        y,
                        h,
                    )
                });
                (
                    table.vertical_scroll_handle.clone(),
                    target,
                    row < table.delegate().view.len(),
                )
            };
            match target {
                // A view swap can strand the target past the list's end;
                // drop the glide instead of animating forever.
                _ if !in_view => self.glide_to = None,
                Some(target)
                    if !panel::glide_step_axis(
                        handle.base_handle(),
                        gpui::Axis::Vertical,
                        target,
                        dt,
                    ) =>
                {
                    self.glide_to = None
                }
                // Not laid out yet, or still moving: keep going.
                _ => window.request_animation_frame(),
            }
        }

        let busy = self.state.library.read(cx).busy().is_some();
        // The "open a folder" call-to-action means the catalog itself holds no
        // tracks, so it keys off the loaded projection, never the view. Off the
        // view it would flash during the initial load (the projection hasn't
        // arrived, the view is transiently empty), and it would wrongly show
        // when a search or filter hides every row. `is_some_and` keeps it off
        // until the projection loads: while None, the empty view stands.
        let catalog_empty = self
            .state
            .library
            .read(cx)
            .projection()
            .is_some_and(|p| p.is_empty());
        let body = if catalog_empty && !busy {
            self.empty_state(cx).into_any_element()
        } else {
            self.track_list().into_any_element()
        };
        // The controls show in the tab bar via title_suffix while the panel
        // shares a group; solo or popped out there's no header at all, so
        // the toolbar renders in the body instead.
        let headerless = self
            .tab_panel
            .as_ref()
            .and_then(|tabs| tabs.upgrade())
            .is_none_or(|tabs| tabs.read(cx).panels_count() < 2);
        // The root must size itself: the dock's tab panel lays the panel view
        // out as a root element (cached, absolute), where flex_1 has no flex
        // parent to grow in and the height would collapse to the content.
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_panel())
            .track_focus(&self.focus)
            // Scopes the workspace's space-bound playback binding out while
            // a type-ahead phrase is mid-flight, the same way the search
            // box's own context does: bindings win over key listeners, so
            // without this a space continuing a phrase would also toggle
            // playback before on_panel_key ever saw the keystroke.
            // While a phrase is up the panel carries its contexts, which
            // scope the workspace's space binding out (only while the
            // phrase is still taking keystrokes) and Root's tab traversal
            // out (for as long as there's a phrase to cycle).
            .when_some(
                panel::type_ahead_context(&self.type_ahead, self.type_ahead_at),
                |d, context| d.key_context(context),
            )
            // A press anywhere in the panel ends the phrase: the cursor
            // has moved by hand, so the cycle it was stepping is stale,
            // and tab belongs back with panel traversal. Capture phase,
            // so rows and tiles that stop the press can't hide it.
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                this.clear_type_ahead(cx);
            }))
            // Tab cycles the live phrase's matches, off the bindings the
            // TypeAhead context above scopes in; with no phrase up, tab
            // stays Root's focus traversal.
            .on_action(cx.listener(|this, _: &TypeAheadNext, _, cx| this.type_step(false, cx)))
            .on_action(cx.listener(|this, _: &TypeAheadPrev, _, cx| this.type_step(true, cx)))
            .on_key_down(
                cx.listener(|this, event, window, cx| this.on_panel_key(event, window, cx)),
            )
            // While click-to-sort is on, the column drag arms on Alt, so
            // the header is rebuilt with or without its grab as the key
            // comes and goes.
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                this.set_alt(event.modifiers.alt, cx);
            }))
            // Any scroll or press over the list counts as browsing; the
            // stamps only restart the idle clock, leaving the scroll and the
            // click to the table underneath, so nothing acts twice.
            .on_scroll_wheel(cx.listener(|this, _: &ScrollWheelEvent, _, cx| {
                this.touch_resume(cx);
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.touch_resume(cx)),
            )
            .when(
                headerless && (self.show_search || self.error.is_some()),
                |d| d.child(self.toolbar(window, cx)),
            )
            .child(div().flex_1().min_h_0().relative().child(body).children(
                panel::type_ahead_overlay(&self.type_ahead_display(), self.type_ahead_at),
            ))
    }
}

/// The column rename window: one input over a header's name, the panel
/// rename window's shape at the column's scale. Edits apply as they're
/// typed, so the header follows along, and Enter or Escape closes.
///
/// Clearing the field puts the registry's label back, so an empty field
/// reads as what it does. A header that draws nothing is asked for with a
/// single space: the value is trimmed before it's stored, so the space
/// lands as an empty name rather than as no name at all.
struct ColumnRenameWindow {
    panel: WeakEntity<LibraryPanel>,
    input: Entity<InputState>,
    /// The shared state, for the window's own backdrop.
    state: AppState,
    backdrop: WindowBackdrop,
    _input_events: Subscription,
    /// This window pumps its own frames, so the backdrop needs its own
    /// wake on a new bake.
    _backdrop_changed: Subscription,
}

impl ColumnRenameWindow {
    fn new(
        panel: WeakEntity<LibraryPanel>,
        state: AppState,
        key: String,
        placeholder: &'static str,
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The registry's label is the placeholder, so an empty field reads
        // as the fallback it is.
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(current)
        });
        // The column is held by key rather than by index: the columns can
        // be reordered or hidden while this window is open.
        let _input_events = cx.subscribe_in(
            &input,
            window,
            move |this: &mut Self, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    // An empty field is the registry's label back. A name
                    // is trimmed before it's stored, so a lone space lands
                    // as an empty name rather than as no name at all.
                    let raw = input.read(cx).value().to_string();
                    let label = (!raw.is_empty()).then(|| raw.trim().to_string());
                    this.panel
                        .update(cx, |panel, cx| {
                            panel.set_column_label(key.clone(), label, cx)
                        })
                        .ok();
                }
                // The name was written as it was typed, so committing is
                // closing.
                InputEvent::PressEnter { .. } => window.remove_window(),
                _ => {}
            },
        );
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        window.focus(&input.read(cx).focus_handle(cx));
        ColumnRenameWindow {
            panel,
            input,
            state,
            backdrop: WindowBackdrop::default(),
            _input_events,
            _backdrop_changed,
        }
    }
}

impl Render for ColumnRenameWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            // Escape leaves the way Enter does: the name is already on the
            // header, so there's nothing here to cancel.
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, window, _| {
                if event.keystroke.key == "escape" {
                    window.remove_window();
                }
            }))
            // The backdrop paints first, under the input, like every other
            // window over the shared state.
            .children(self.backdrop.layer(&self.state.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .bg(palette::bg_elevated())
                    .p(tokens::SPACE_MD)
                    .child(settings_ui::section(
                        rox_i18n::t!("library-column-rename-name"),
                        None,
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_XS)
                            .child(Input::new(&self.input).w_full())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(palette::text_muted())
                                    .child(rox_i18n::t!("library-column-rename-note")),
                            ),
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rox_library::projection::FilterSet;
    use rox_library::{store, TrackRow};

    /// A track row carrying only what the view pass and the windowing
    /// read; everything else stays at its neutral default.
    fn track(path: &str, album_artist: &str, album: &str, track_no: u16) -> TrackRow {
        TrackRow {
            title_sort: String::new(),
            artist_sort: String::new(),
            album_artist_sort: String::new(),
            album_sort: String::new(),
            sub: 0,
            cue: None,
            path: path.into(),
            title: path.into(),
            artist: album_artist.into(),
            album_artist: album_artist.into(),
            album: album.into(),
            genre: String::new(),
            year: 2000,
            disc_no: 1,
            track_no,
            duration_ms: 1000,
            codec: "flac".into(),
            bitrate_kbps: 900,
            sample_rate_hz: 44100,
            bit_depth: 16,
            rating: 0,
            replay_gain: Default::default(),
            bpm: None,
            size: 0,
            mtime: 0,
        }
    }

    /// A projection over an in-memory database, the same load path the
    /// catalog runs.
    fn projection(rows: &[TrackRow]) -> Arc<Projection> {
        let mut conn = rox_library::rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, rows).unwrap();
        Arc::new(Projection::load_serial(&conn, false).unwrap())
    }

    /// The window the old pass produced: every track row in the view
    /// listed, then a slice of it taken around the click. What
    /// [`play_window`] has to keep answering without the list.
    fn window_by_listing(view: &[Row], ix: usize, cap: usize) -> Option<(Vec<usize>, usize)> {
        let tracks: Vec<usize> = (0..view.len())
            .filter(|&i| matches!(view[i], Row::Track(_)))
            .collect();
        let clicked = tracks.iter().position(|&i| i == ix)?;
        let mut lo = clicked.saturating_sub(cap / 2);
        lo = lo.min(tracks.len().saturating_sub(cap));
        let hi = (lo + cap).min(tracks.len());
        Some((tracks[lo..hi].to_vec(), clicked - lo))
    }

    /// A view of `tracks` track rows with a header block every `run` of
    /// them, so the windowing has non-track rows to walk over.
    fn view_with_heads(tracks: usize, run: usize) -> Vec<Row> {
        let mut view = Vec::new();
        for i in 0..tracks {
            if i % run == 0 {
                view.push(Row::Head((i / run) as u32, 0));
                view.push(Row::Head((i / run) as u32, 1));
            }
            view.push(Row::Track(i as u32));
        }
        view
    }

    /// Walking out from the click lands on exactly the slice the full
    /// listing did: same rows, same order, same offset for the clicked
    /// one, wherever in the view it sits and however the cap compares to
    /// the view's length.
    #[test]
    fn the_play_window_matches_the_full_listing() {
        for (tracks, run) in [(1, 1), (7, 3), (40, 5), (101, 7)] {
            let view = view_with_heads(tracks, run);
            for cap in [1, 2, 3, 10, 40, 500] {
                for (ix, _) in view
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| matches!(row, Row::Track(_)))
                {
                    assert_eq!(
                        play_window(&view, ix, cap),
                        window_by_listing(&view, ix, cap),
                        "tracks {tracks}, run {run}, cap {cap}, row {ix}"
                    );
                }
            }
        }
    }

    /// A press on a header row or past the end plays nothing.
    #[test]
    fn the_play_window_needs_a_track_row() {
        let view = view_with_heads(4, 2);
        assert!(play_window(&view, 0, 10).is_none());
        assert!(play_window(&view, view.len(), 10).is_none());
    }

    /// The window never runs past the budget, and fills it whenever the
    /// view has the rows to fill it with.
    #[test]
    fn the_play_window_fills_the_budget() {
        let view = view_with_heads(100, 4);
        for ix in (0..view.len()).filter(|&i| matches!(view[i], Row::Track(_))) {
            let (rows, start) = play_window(&view, ix, 20).expect("a track row");
            assert_eq!(rows.len(), 20);
            assert_eq!(rows[start], ix);
        }
    }

    /// The rows a fixture's view holds, computed the way the panel used to
    /// compute them inline.
    fn view_directly(inputs: &ViewInputs) -> (Arc<Vec<Row>>, Vec<Group>) {
        let key = |projection: &Projection, row: u32| -> u64 {
            let i = row as usize;
            (projection.album_artist[i] as u64) << 32 | projection.album[i] as u64
        };
        view::view_for(
            &inputs.projection,
            inputs.order.clone(),
            &ViewSpec {
                query: &inputs.query,
                filter: &inputs.filter,
                similar: None,
                sort: inputs.sort,
                grouping: inputs.head_rows.map(|head_rows| Grouping {
                    head_rows,
                    pre_sort: None,
                    key: &key,
                    discs: true,
                }),
            },
        )
    }

    fn inputs(projection: &Arc<Projection>, query: &str, head_rows: Option<u8>) -> ViewInputs {
        ViewInputs {
            projection: projection.clone(),
            order: Arc::new(projection.sort_canonical()),
            query: query.to_string(),
            filter: FilterSet::default(),
            similar: None,
            sort: None,
            group_by: GroupBy::Album,
            head_rows,
        }
    }

    /// The pass hands back the same view off the UI thread as it did on
    /// it: same rows in the same order, same groups, for a grouped view, a
    /// flat one, and a search. Running it on a plain thread also pins the
    /// inputs as `Send`, which is what lets the executor take them at all.
    #[test]
    fn the_background_pass_computes_the_same_view() {
        let p = projection(&[
            track("/m/a1.flac", "A", "One", 1),
            track("/m/a2.flac", "A", "One", 2),
            track("/m/b1.flac", "B", "Two", 1),
            track("/m/b2.flac", "B", "Two", 2),
            track("/m/c1.flac", "C", "Three", 1),
        ]);
        for (query, head_rows) in [("", Some(2u8)), ("", None), ("b", Some(2u8))] {
            let direct = view_directly(&inputs(&p, query, head_rows));
            let sent = inputs(&p, query, head_rows);
            let off_thread = std::thread::spawn(move || compute_rows(&sent))
                .join()
                .expect("the pass");
            assert_eq!(*off_thread.0, *direct.0, "rows for {query:?}");
            let shape = |groups: &[Group]| -> Vec<(u32, u32, u64)> {
                groups
                    .iter()
                    .map(|g| (g.first, g.tracks, g.total_ms))
                    .collect()
            };
            assert_eq!(
                shape(&off_thread.1),
                shape(&direct.1),
                "groups for {query:?}"
            );
        }
    }
}
