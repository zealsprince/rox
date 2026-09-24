//! The dockable library panel, a view over the shared catalog in
//! `crate::catalog`. The catalog owns the library database and only hands out
//! the in-memory projection. Each panel keeps its own search config, so a
//! duplicate filters independently. Double click plays on the shared player;
//! a single click selects, and the selection publishes app-wide.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, App, ClickEvent, Context, Div, Entity, EventEmitter, FocusHandle, Focusable,
    KeyDownEvent, ModifiersChangedEvent, MouseButton, ScrollStrategy, ScrollWheelEvent,
    SharedString, Stateful, Subscription, WeakEntity, Window, WindowHandle, div, prelude::*, px,
    rems,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::table::{Column, ColumnSort, Table, TableDelegate, TableEvent, TableState};
use gpui_component::{Icon, IconName, Root, Side, Sizable, Size};
use rox_dock::{Panel, PanelEvent, PanelInfo, PanelState, TabPanel};
use rox_panel_api::actions::{TypeAheadNext, TypeAheadPrev};

use rox_core::fmt::{fmt_ago, fmt_ms, fmt_num};
use rox_core::{QUEUE_CAP, SHUFFLE_SEED};
use rox_library::cue::TrackKey;
use rox_library::projection::{Projection, QUERY_FIELDS, QueryField};
use rox_library::view::{self, Group, Grouping, Row, ViewSpec};
use rox_services::backdrop::WindowBackdrop;

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::continuation;
use crate::design::{palette, tokens};
use crate::group_head::{
    self, ArtSide, HeadPiece, Headers, MOSAIC, TileFace, effective_head_lines,
};
use crate::panel::{self, AppState, PanelChrome, ResumeIdle, ScrubState};
use crate::panel_settings;
use crate::query::search::{SearchBox, SearchEvent};
use crate::query::shared_query::{QueryFilter, QuerySource, SharedQueryEvent};
use crate::selection::SelectionEvent;
use crate::settings::GainModeSetting;
use crate::settings::ui as settings_ui;
use crate::thumbs::Thumb;
use crate::track_ui::track_cells;
use crate::track_ui::track_drag::{PlayDrag, PlayDragPreview};

/// Matches the panel frame sliders' rounding scale.
const ART_ROUNDING_MAX: f32 = 24.;

const PAGE_ROWS: isize = 25;

/// Long enough that typing a word runs one view pass, not one per letter.
const VIEW_DEBOUNCE: Duration = Duration::from_millis(100);

mod columns;

pub use columns::LibraryConfig;
use columns::*;

/// A depth or rate the group disagrees on goes over as 0, which `quality`
/// drops.
fn group_quality(group: &Group, projection: &Projection) -> String {
    group_head::quality(
        group.codec_name(projection),
        group.min_kbps,
        group.max_kbps,
        group.bit_depth.unwrap_or(0),
        group.sample_rate_hz.unwrap_or(0),
    )
}

/// Everything one view pass reads, owned so it can run on the background
/// executor. The catalog swaps the projection and order `Arc`s whole and never
/// patches them, so a pass in flight stays consistent.
struct ViewInputs {
    projection: Arc<Projection>,
    /// Lets the rows be refused if the catalog rebuilt while the pass ran.
    projection_gen: u64,
    order: Arc<Vec<u32>>,
    query: String,
    filter: rox_library::projection::FilterSet,
    similar: Option<(Arc<HashMap<i64, f32>>, bool)>,
    sort: Option<(rox_library::projection::SortKey, bool)>,
    group_by: GroupBy,
    group_search_results: bool,
    head_rows: Option<u8>,
}

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
    let grouping = inputs
        .head_rows
        .filter(|_| inputs.query.is_empty() || inputs.group_search_results)
        .map(|head_rows| Grouping {
            head_rows,
            // Search returns projection order, so without this pre-sort one
            // album can split into several headers. A column sort ignores it.
            pre_sort: if inputs.query.is_empty() {
                group_by.sort()
            } else {
                Some(group_by.search_sort())
            },
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

/// Up to `cap` track rows around `ix` for a click to play through, at most
/// half of them behind it, plus the clicked row's offset. Walks out from the
/// click because listing every track row first is tens of megabytes on a big
/// library.
fn play_window(view: &[Row], ix: usize, cap: usize) -> Option<(Vec<usize>, usize)> {
    if cap == 0 || !matches!(view.get(ix)?, Row::Track(_)) {
        return None;
    }
    let track_rows =
        |range: std::ops::Range<usize>| range.filter(|&i| matches!(view[i], Row::Track(_)));
    let mut behind: Vec<usize> = track_rows(0..ix).rev().take(cap / 2).collect();
    let ahead: Vec<usize> = track_rows(ix + 1..view.len())
        .take(cap - behind.len() - 1)
        .collect();
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

/// The clicked track, then `count` track rows sampled uniformly from the rest
/// of the view. A window would only ever mix the first few artists.
fn shuffle_seed(view: &[Row], ix: usize, count: usize) -> Vec<usize> {
    let head = matches!(view.get(ix), Some(Row::Track(_))).then_some(ix);
    let rest = (0..view.len()).filter(|&i| Some(i) != head && matches!(view[i], Row::Track(_)));
    let mut rows = Vec::with_capacity(count + 1);
    rows.extend(head);
    rows.extend(rox_playback::engine::reservoir(rest, count));
    rows
}

/// Swap a finished pass into the table unless a newer one was scheduled.
/// True when the rows landed.
fn install_view(
    table: &mut TableState<TrackTable>,
    generation: u64,
    projection_gen: u64,
    view: Arc<Vec<Row>>,
    groups: Vec<Group>,
    cx: &mut Context<TableState<TrackTable>>,
) -> bool {
    if table.delegate().view_gen != generation {
        return false;
    }
    // Stale projection. The refresh after its swap is computing the right rows.
    if projection_gen != table.delegate().state.library.read(cx).projection_gen() {
        return false;
    }
    // Selection indices point into the old view. A refresh isn't an explicit
    // pick, so the shared selection stays.
    let delegate = table.delegate_mut();
    delegate.view = view;
    delegate.groups = groups;
    delegate.view_projection = projection_gen;
    delegate.selected.clear();
    delegate.sel_gen += 1;
    delegate.anchor = None;
    delegate.cursor = None;
    delegate.locate_playing(cx);
    table.clear_selection(cx);
    cx.notify();
    true
}

struct TrackTable {
    state: AppState,
    panel: WeakEntity<LibraryPanel>,
    view: Arc<Vec<Row>>,
    /// What header rows index. Always swapped together with `view`.
    groups: Vec<Group>,
    /// From here to `compact_plays`, knobs copied from the panel so the view
    /// pass and the render can read them off the delegate.
    headers: Headers,
    group_by: GroupBy,
    group_search_results: bool,
    row_height: f32,
    row_spacing: f32,
    /// One header line's height. A block spans however many rows its lines need.
    head_height: f32,
    head_text: f32,
    art_rounding: f32,
    art_side: ArtSide,
    art_margin: f32,
    header_gap_above: f32,
    header_gap_below: f32,
    header_art: bool,
    portrait_circle: bool,
    genre_face: TileFace,
    header_flush: bool,
    /// Never empty.
    head_lines: Vec<Vec<HeadPiece>>,
    compact_plays: bool,
    /// Indices into `view`, track rows only. Cleared when the view swaps.
    selected: HashSet<usize>,
    /// Where the next shift-click extends from.
    anchor: Option<usize>,
    /// The keyboard cursor. Follows clicks.
    cursor: Option<usize>,
    columns: Vec<Column>,
    /// User-renamed headers, kept so a language switch can tell them from
    /// resolved labels. An empty value draws the header blank.
    labels: HashMap<String, String>,
    /// While set, the widget's own sorting is off, and the header click, the
    /// arrow, and the Alt-held column drag all run from here.
    sort_on_click: bool,
    /// The language the column labels were resolved in. They don't follow a
    /// language switch on their own.
    columns_locale: &'static str,
    /// Column key and whether it descends. None is the canonical order.
    sort: Option<(SharedString, bool)>,
    playing_id: Option<i64>,
    playing_row: Option<usize>,
    favourites: HashSet<i64>,
    /// Similar-column scores against the playing track. Scored on a background
    /// thread, never in a paint: the pass is tens of milliseconds on a large
    /// library.
    similar: Arc<HashMap<i64, f32>>,
    /// The track and acoustic model `similar` was scored against. None while
    /// it's empty, so the next look retries.
    similar_anchor: Option<(i64, String)>,
    /// Cached so the thumbnail lookup doesn't query the catalog every frame.
    cover_paths: HashMap<i64, Option<PathBuf>>,
    /// Cached because a row's `on_drag` value is built every frame.
    drag_keys: HashMap<i64, Option<TrackKey>>,
    /// Bumped on every selection change. Keys the `drag_set` cache.
    sel_gen: u64,
    /// Bumped per scheduled pass. A pass whose number no longer matches is
    /// dropped on arrival.
    view_gen: u64,
    /// Which projection build the indices in `view` and `groups` belong to. A
    /// rebuild renumbers every row and the matching view pass lands a frame or
    /// more later, so in that gap a read past the end of a shrunk projection
    /// panics. Every read of a view row goes through [`TrackTable::projection`].
    view_projection: u64,
    /// Refreshed at most every half minute instead of a `SystemTime::now` per
    /// cell per frame.
    added_now: i64,
    added_now_at: Instant,
    /// Built once per selection change and shared by every selected row's drag.
    drag_set: Option<DragSet>,
}

/// (generation, keys a drop plays, catalog ids a playlist drop stores).
type DragSet = (u64, Arc<[TrackKey]>, Arc<[i64]>);

impl TrackTable {
    /// Called by the widget's sort hook, and by the header click with
    /// click-to-sort on. Schedules the pass instead of refreshing through the
    /// panel, which would re-enter the table mid-update. Sorting ten million rows
    /// takes a quarter second on integer ranks and near a second by title.
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
        let projection_gen = inputs.projection_gen;
        let panel = self.panel.clone();
        cx.spawn(async move |table, cx| {
            let (view, groups) = cx
                .background_executor()
                .spawn(async move { compute_rows(&inputs) })
                .await;
            let installed = table
                .update(cx, |table, cx| {
                    install_view(table, generation, projection_gen, view, groups, cx)
                })
                .unwrap_or(false);
            // A sort is a landing too. Without this the restored scroll and a
            // pending follow wait for some unrelated refresh to yank the list.
            if installed {
                panel
                    .update(cx, |panel, cx| panel.on_view_installed(cx))
                    .ok();
            }
        })
        .detach();
        cx.notify();
    }

    /// Only the wording changes: order, widths, and sort are the user's layout.
    /// A language switch arrives as nothing but a repaint.
    fn reword_columns(&mut self) {
        let locale = rox_i18n::locale();
        if self.columns_locale == locale {
            return;
        }
        self.columns_locale = locale;
        columns::reword(&mut self.columns, &self.labels);
    }

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

    /// The live projection, only while it's the build `view` was computed over.
    /// None in the gap before this panel's pass lands, so cells draw empty for
    /// a frame or two instead of indexing rows that no longer exist.
    fn projection<'a>(&self, cx: &'a App) -> Option<&'a Arc<Projection>> {
        let library = self.state.library.read(cx);
        if library.projection_gen() != self.view_projection {
            return None;
        }

        library.projection()
    }

    fn track_at(&self, ix: usize) -> Option<u32> {
        match self.view.get(ix) {
            Some(&Row::Track(row)) => Some(row),
            _ => None,
        }
    }

    /// A grab inside a multi-selection drags the whole set in view order,
    /// otherwise just this row. Resolves through `keys_for` like the play
    /// actions, so a drop enqueues exactly what they would.
    fn drag_payload(&mut self, ix: usize, cx: &App) -> Option<PlayDrag> {
        let projection = self.projection(cx).cloned()?;
        let title = self
            .track_at(ix)
            .map(|row| projection.resolve(row).title.to_string())
            .unwrap_or_default();
        let (keys, ids): (Arc<[TrackKey]>, Arc<[i64]>) = if self.selected.len() > 1
            && self.selected.contains(&ix)
        {
            if self.drag_set.as_ref().map(|(generation, ..)| *generation) != Some(self.sel_gen) {
                let mut rows: Vec<usize> = self.selected.iter().copied().collect();
                rows.sort_unstable();
                let (set, ids) = self.resolve_drag_keys(&rows, &projection, cx);
                self.drag_set = Some((self.sel_gen, set.into(), ids.into()));
            }
            self.drag_set
                .as_ref()
                .map(|(_, set, ids)| (set.clone(), ids.clone()))?
        } else {
            let (set, ids) = self.resolve_drag_keys(&[ix], &projection, cx);
            (set.into(), ids.into())
        };
        if keys.is_empty() {
            return None;
        }
        Some(PlayDrag {
            keys,
            ids,
            title: title.into(),
        })
    }

    fn resolve_drag_keys(
        &mut self,
        rows: &[usize],
        projection: &Projection,
        cx: &App,
    ) -> (Vec<TrackKey>, Vec<i64>) {
        let ids: Vec<i64> = rows
            .iter()
            .filter_map(|&i| self.track_at(i))
            .map(|row| projection.db_id[row as usize])
            .collect();
        let mut keys = Vec::with_capacity(ids.len());
        for &id in &ids {
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

        (keys, ids)
    }

    /// Bounces off the ends. None only when the view holds no tracks.
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

    /// None when `ix` isn't a header line. Any line of a block counts.
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

    fn line_px(&self) -> gpui::Pixels {
        px(self.head_height) * palette::row_scale()
    }

    fn gap_above_px(&self) -> gpui::Pixels {
        px(self.header_gap_above) * palette::row_scale()
    }

    fn gap_below_px(&self) -> gpui::Pixels {
        px(self.header_gap_below) * palette::row_scale()
    }

    fn art_margin_px(&self) -> gpui::Pixels {
        px(self.art_margin) * palette::row_scale()
    }

    /// Follows the height slider from the stock 1 rem, clamped so a dense list
    /// stays legible.
    fn row_font_scale(&self) -> f32 {
        (self.row_height / ROW_HEIGHT_STOCK).clamp(0.8, 1.8)
    }

    /// Free of the line height, so the art grows without dragging the text.
    fn head_font_scale(&self) -> f32 {
        self.head_text / HEAD_TEXT_STOCK
    }

    fn head_rows(&self) -> u8 {
        // One row per line. The table takes this delegate's heights, so
        // nothing rounds to whole rows.
        self.head_lines.len().clamp(1, u8::MAX as usize) as u8
    }

    /// The lines' full height less the margin, so the art squares off against
    /// the text.
    fn tile_side(&self) -> gpui::Pixels {
        let side = self.line_px() * self.head_lines.len() as f32 - self.art_margin_px() * 2.;
        if side < px(0.) { px(0.) } else { side }
    }

    fn circled(&self) -> bool {
        self.group_by == GroupBy::Artist && self.portrait_circle
    }

    fn tile_rounding(&self) -> f32 {
        if self.circled() {
            f32::from(self.tile_side()) / 2.
        } else {
            self.art_rounding
        }
    }

    /// Year and details stay on because the composed lines already hold those
    /// choices.
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

    /// A header's cover tile, painted whole by each of the block's rows at
    /// `lift` with the last draw winning. Pending and missing share one
    /// placeholder, so an arriving cover doesn't shift the text.
    fn group_tile(
        &mut self,
        g: u32,
        lift: gpui::Pixels,
        cx: &mut Context<TableState<Self>>,
    ) -> AnyElement {
        if self.group_by == GroupBy::Genre {
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
                let projection = self.projection(cx);
                self.groups
                    .get(g as usize)
                    .zip(projection)
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

    /// None falls back to the cover. An arriving face notifies the service, and
    /// the panel's subscription repaints the tile.
    fn group_portrait(&mut self, g: u32, cx: &mut Context<TableState<Self>>) -> Option<Thumb> {
        if self.group_by != GroupBy::Artist {
            return None;
        }
        let name = {
            let projection = self.projection(cx)?;
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

    /// Cached on the group. Empty for the unknown bucket, which keeps the
    /// placeholder instead of whichever loose track's art loads first.
    fn group_art_paths(&mut self, g: u32, cx: &mut Context<TableState<Self>>) -> Vec<PathBuf> {
        if let Some(paths) = self
            .groups
            .get(g as usize)
            .and_then(|group| group.art.clone())
        {
            return paths;
        }
        let paths = {
            let projection = self.projection(cx);
            let ids: Vec<i64> = self
                .groups
                .get(g as usize)
                .zip(projection)
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
            self.state
                .library
                .read(cx)
                .paths_for(&ids)
                .unwrap_or_default()
        };
        if let Some(group) = self.groups.get_mut(g as usize) {
            group.art = Some(paths.clone());
        }
        paths
    }

    /// The first track of each of the run's first [`MOSAIC`] distinct tagged
    /// albums, matching the genre grid.
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

    /// One table row of a header block. Every row of the block paints the whole
    /// canvas unclipped, shifted up past the rows above, so the last paint shows
    /// and the block draws seamlessly at any line and row height. The year
    /// grouping has no one image, so it alone goes without a tile.
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
        let lines = self.head_lines.len().max(1);
        let first = line == 0;
        let last = line as usize + 1 >= lines;
        let line_px = self.line_px();
        let gap_above = if first { self.gap_above_px() } else { px(0.) };
        let gap_below = if last { self.gap_below_px() } else { px(0.) };
        // The first row drops the tile past its gap, the rest lift it past the
        // lines already painted.
        let lift = if first {
            -self.gap_above_px()
        } else {
            line_px * line as f32
        };
        let tile = has_tile.then(|| self.group_tile(g, lift, cx));
        // A line-tall square has no room for the genre mosaic.
        let inline_art = with_art && self.head_lines.iter().any(|l| l.contains(&HeadPiece::Art));
        let mut head = match (self.groups.get(g as usize), self.projection(cx)) {
            (Some(group), Some(projection)) => {
                let v = projection.resolve(group.first);
                let name = match self.group_by {
                    GroupBy::Album | GroupBy::Artist => {
                        // Migrated rows have no album artist until a
                        // rescan, so the first track's artist stands in.
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
                // Follows whichever artist field stood in for the name.
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
        // The panel body already painted the list surface. Painting it again
        // lays a second coat, which shows once surfaces go translucent.
        let tinted = bg != palette::bg_root();
        // A row with no gap tints its own background so the block's bottom
        // hairline draws over it. Edge rows tint a child slice instead.
        let strip = self
            .head_lines
            .get(line as usize)
            .map(|pieces| group_head::line_content(pieces, &head, &look, expanded));
        div()
            .id(("row", row_ix))
            .cursor_pointer()
            // No border inside a block. The width stays, so rows keep their height.
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

    /// Full-width like the header rows, so it stays put when wide column sets
    /// scroll sideways.
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

    /// Split a leading `field:` pin ([`QUERY_FIELDS`]) off a type-ahead phrase.
    /// Unknown and numeric-only fields fall through, so the phrase reads literally.
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
            QueryField::Source => "source",
            QueryField::Year
            | QueryField::Folder
            | QueryField::Rating
            | QueryField::Plays
            | QueryField::Added => return None,
        };
        Some((key, rest))
    }

    /// From the cursor on, wrapping. A plain phrase matches word starts in title,
    /// artist, album artist, and album. Genre and codec need a `field:` pin,
    /// since they'd match nearly every row.
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

    fn find_step(&self, prefix: &str, back: bool, cx: &App) -> Option<usize> {
        self.find_in(
            panel::type_ahead_scan(self.view.len(), self.cursor, back),
            prefix,
            cx,
        )
    }

    fn find_in(&self, order: impl Iterator<Item = usize>, prefix: &str, cx: &App) -> Option<usize> {
        let projection = self.projection(cx)?;
        let pin = Self::type_ahead_pin(prefix);
        order.into_iter().find(|&ix| {
            let Some(row) = self.track_at(ix) else {
                return false;
            };
            let v = projection.resolve(row);
            match pin {
                Some((field, needle)) => {
                    // A server's stored source is a digest, so match its label.
                    let source;
                    let text = match field {
                        "title" => v.title,
                        "artist" => v.artist,
                        "album_artist" => v.album_artist,
                        "album" => v.album,
                        "genre" => v.genre,
                        "source" => {
                            source = rox_library::cue::source_label(v.source);
                            source.as_str()
                        }
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

    /// One scan per view swap or track change, never per frame.
    fn locate_playing(&mut self, cx: &App) {
        let row = self.playing_id.and_then(|id| {
            let projection = self.projection(cx)?;
            self.view
                .iter()
                .position(|&row| matches!(row, Row::Track(r) if projection.db_id[r as usize] == id))
        });
        self.playing_row = row;
    }

    /// None while the catalog has no projection yet.
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
            projection_gen: library.projection_gen(),
            order: library.order(),
            query: query.to_string(),
            filter: filter.clone(),
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
            group_search_results: self.group_search_results,
            head_rows: (self.headers != Headers::Off).then(|| self.head_rows()),
        })
    }

    /// Runs mid table update, so the panel's `dropdown_menu` must not read the
    /// table entity at build time. Its click handlers may.
    fn panel_menu(&self, menu: PopupMenu, window: &mut Window, cx: &mut App) -> PopupMenu {
        let Some(panel) = self.panel.upgrade() else {
            return menu;
        };
        panel.update(cx, |panel, cx| panel.dropdown_menu(menu, window, cx))
    }

    fn publish_selection(&self, cx: &mut App) {
        let Some(projection) = self.projection(cx).cloned() else {
            return;
        };
        let mut rows: Vec<usize> = self.selected.iter().copied().collect();
        rows.sort_unstable();
        let ids: Vec<i64> = rows
            .iter()
            .filter_map(|&ix| self.track_at(ix))
            .map(|row| projection.db_id[row as usize])
            .collect();
        // The panel's id, which a scoped drawer and a selection-following view
        // match against.
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

    /// A header block's rows compose one canvas, so a per-row stripe or hover
    /// wash would band it.
    fn plain_row(&self, row_ix: usize) -> bool {
        self.track_at(row_ix).is_some()
    }

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

    /// Fingerprint of everything `row_height` reads. The table rebuilds its size
    /// cache when it moves.
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

    /// The header cell, with a right-click menu to rename it and toggle columns.
    /// The table's own menu builds empty over the header, so the two don't
    /// stack. With click-to-sort on, the sort cycle runs here because the
    /// widget's `perform_sort` is private.
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
                        // Alt is the column drag's grab, never a sort.
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

    /// The widget has already advanced the column's cycle in its own state.
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
        // The table has no row-spanning cell, so header lines hang off the row
        // itself, outside the horizontally scrolled cells.
        match self.view.get(row_ix).copied() {
            Some(Row::Head(g, line)) => return self.render_head_row(row_ix, g, line, cx),
            Some(Row::Disc(disc)) => return self.render_disc_row(row_ix, disc),
            _ => {}
        }
        // Selection matches the widget's own focus wash. The playing row takes a
        // faint highlight so it stays apart from it.
        let selected = self.selected.contains(&row_ix);
        let drag = self.drag_payload(row_ix, cx);
        div()
            // Group bounds resolve innermost-first, so the shared name still
            // scopes each cell's group_hover to its own row.
            .group(track_cells::ROW_GROUP)
            .id(("row", row_ix))
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

    /// A right click outside the selection reselects that row first, and a
    /// header selects its whole group. The panel's own menu goes at the end:
    /// the panel body hands its right-click to the table, so this is the only
    /// menu a click over the list opens.
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
        // Resolved now, so the editor gets this set even if another panel
        // publishes over the shared selection first.
        let ids: Vec<i64> = self
            .projection(cx)
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
        // A single row plays through the view like a double click. A set or a
        // group queues exactly the highlighted rows.
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
        // Single track rows only: a header is already a whole album, and a set
        // has no one album or artist to filter by.
        let menu = if album.is_none() && rows.len() == 1 {
            let (jump_album, jump_artist) = self
                .projection(cx)
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
            // Only once the pass has described something. The switch alone
            // doesn't build the vectors.
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
        // Header rows draw in render_tr, so their cells stay empty.
        let Some(row) = self.track_at(row_ix) else {
            return div().into_any_element();
        };
        let Some(projection) = self.projection(cx).cloned() else {
            return div().into_any_element();
        };
        let v = projection.resolve(row);
        let playing = self.playing_row == Some(row_ix);
        let readings = crate::settings::show_readings();
        let cell = div().truncate();
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
            // The delegate's own row height, so the cover grows with the knob.
            return crate::track_ui::track_columns::cover_cell(&thumb, self.row_height)
                .into_any_element();
        }
        let cell = match key.as_ref() {
            "track" => cell
                .text_color(palette::text_muted())
                .child(fmt_num(v.track_no)),
            // Name columns add a reading after names this alphabet can't sound
            // out. The sort columns show that string alone.
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
            // No fallback to the display name: which rows carry the tag is the
            // point of the column.
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
            "source" => cell
                .text_color(palette::text_muted())
                .child(SharedString::from(rox_library::cue::source_label(v.source))),
            "bitrate" => cell
                .text_color(palette::text_muted())
                .child(fmt_num(v.bitrate_kbps)),
            "sample_rate" => cell
                .text_color(palette::text_muted())
                .child(SharedString::from(group_head::khz(v.sample_rate_hz))),
            "bit_depth" => cell
                .text_color(palette::text_muted())
                .child(fmt_num(v.bit_depth as u16)),
            "duration" => cell
                .text_color(palette::text_muted())
                .child(SharedString::from(fmt_ms(v.duration_ms))),
            // The tag as written, signed. Don't fold in the preamp or the peak
            // clamp, or the file's own number moves with a slider.
            "gain" => match projection
                .gain_db(row, crate::settings::gain_mode() == GainModeSetting::Album)
            {
                Some(db) => {
                    // The locale formatter can't force a sign.
                    let sign = if db.is_sign_negative() { "-" } else { "+" };
                    let magnitude = rox_i18n::format::format_float(f64::from(db.abs()), 2);
                    cell.text_color(palette::text_muted())
                        .child(SharedString::from(format!("{sign}{magnitude}")))
                }
                None => cell,
            },
            // Whole beats: the fraction is estimator noise.
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
            // The raw cosine, since this column is for judging the vectors.
            "similar" => match self.similar.get(&projection.db_id[row as usize]) {
                Some(score) => cell
                    .text_color(palette::text_muted())
                    .child(SharedString::from(rox_i18n::format::format_float(
                        f64::from(*score),
                        2,
                    ))),
                None => cell,
            },
            // The compact face is CaTRoX's "1|" playlist tick.
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
            "added" => cell
                .text_color(palette::text_muted())
                .child(if v.added <= 0 {
                    SharedString::default()
                } else {
                    SharedString::from(fmt_ago(self.added_now() - v.added))
                }),
            _ => cell,
        };
        // gpui lays the line box (phi line height) from the cell top, so short
        // rows chop descenders. Centering splits the overshoot evenly.
        div()
            .size_full()
            .flex()
            .flex_row()
            .items_center()
            .child(cell.w_full().min_w_0())
            .into_any_element()
    }

    /// The table calls this before reordering its own col_groups, so cell
    /// rendering stays aligned.
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

    /// Quiet on no hits. An empty library never gets here: the panel draws its
    /// own empty state.
    fn render_empty(
        &mut self,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
    }
}

/// One browse view over the shared catalog, with its own query and row order.
pub struct LibraryPanel {
    state: AppState,
    table: Entity<TableState<TrackTable>>,
    query: String,
    /// Kept apart from the search input's focus so activating the tab doesn't
    /// send keystrokes into the query.
    focus: FocusHandle,
    search: Entity<SearchBox>,
    /// While hidden, the query keeps its text but stops applying.
    show_search: bool,
    /// While global, `query` keeps the panel's own text dormant for the switch
    /// back.
    query_source: QuerySource,
    /// The active source's text needs to go into the box, which takes a window,
    /// so the next render applies it.
    resync_box: bool,
    /// Runtime only: a restore re-pins from whatever is picked then.
    selection_ids: Vec<i64>,
    /// A failed play, shown until the catalog updates.
    error: Option<SharedString>,
    /// The change detector. The player notifies every pump tick, so everything
    /// up to this compare stays cheap.
    playing_key: Option<TrackKey>,
    type_ahead: String,
    type_ahead_at: Option<std::time::Instant>,
    /// The catalog loads after the panel builds, so the first non-empty view
    /// consumes this.
    restore_scroll: Option<usize>,
    follow_playing: bool,
    smooth_follow: bool,
    /// So a refresh that leaves the playing track in place doesn't scroll again.
    followed_row: Option<usize>,
    /// The playing row only exists once the rebuilt view lands, so the follow
    /// waits for it.
    follow_on_view: bool,
    resume_playing: bool,
    resume_idle: ResumeIdle,
    glide_to: Option<usize>,
    glide_tick: Instant,
    row_height: f32,
    head_height: f32,
    row_spacing: f32,
    head_text: f32,
    row_scrub: ScrubState,
    head_scrub: ScrubState,
    row_spacing_scrub: ScrubState,
    head_text_scrub: ScrubState,
    /// Also on the delegate. Kept here so the dropdown builds without reading
    /// the table entity, since the row context menu builds mid-table-update.
    headers: Headers,
    group_by: GroupBy,
    group_search_results: bool,
    /// Copied off the delegate for the same reason as `headers`.
    columns_shown: HashSet<String>,
    /// Process statics with nothing to subscribe to. Their changes repaint every
    /// window, which is when [`LibraryPanel::watch_similarity`] compares.
    similar_watch: (String, bool),
    art_rounding: f32,
    art_scrub: ScrubState,
    art_side: ArtSide,
    art_margin: f32,
    art_margin_scrub: ScrubState,
    header_gap_above: f32,
    header_gap_above_scrub: ScrubState,
    header_gap_below: f32,
    header_gap_below_scrub: ScrubState,
    /// The one readout being typed into across the settings sliders.
    value_edit: panel::ValueEdit,
    header_art: bool,
    portrait_circle: bool,
    genre_face: TileFace,
    header_flush: bool,
    header_compact: Vec<HeadPiece>,
    /// Always [`HEAD_LINE_SLOTS`] entries. An empty slot drops out of the
    /// rendered block.
    header_lines: Vec<Vec<HeadPiece>>,
    /// UI only and not persisted: the fixed slots can't say "added but empty".
    header_lines_shown: usize,
    compact_plays: bool,
    stripes: bool,
    row_borders: bool,
    column_headers: bool,
    sort_on_click: bool,
    /// Reopening on the same header focuses it, another header closes it first.
    /// A closed window's handle fails to activate, so the next open replaces it.
    column_rename: Option<(String, WindowHandle<Root>)>,
    chrome: PanelChrome,
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Whether this panel is solo decides where the toolbar renders.
    _tabs_changed: Option<Subscription>,
    _library_changed: Subscription,
    _table_events: Subscription,
    _search_events: Subscription,
    _query_changed: Subscription,
    _selection_changed: Subscription,
    _player_changed: Subscription,
    _thumbs_changed: Subscription,
    _portraits_changed: Subscription,
    /// Drops the phrase on blur, so tab goes back to walking panels.
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
                // Ratings and listens only repaint. Re-sorting a rating-sorted
                // view here would yank the row out from under the cursor.
                if matches!(event, LibraryEvent::Rated | LibraryEvent::Played) {
                    this.table.update(cx, |_, cx| cx.notify());
                    return;
                }
                // Imported play counts only reorder a plays-sorted view.
                if matches!(event, LibraryEvent::PlaysReloaded) {
                    let sorted_on_plays = this
                        .table
                        .read(cx)
                        .delegate()
                        .sort
                        .as_ref()
                        .is_some_and(|(key, _)| key == "plays");
                    if sorted_on_plays {
                        this.refresh_view(cx);
                    } else {
                        this.table.update(cx, |_, cx| cx.notify());
                    }
                    return;
                }
                if matches!(event, LibraryEvent::PlaylistsChanged) {
                    this.reload_favourites(cx);
                    return;
                }
                this.error = None;
                // The catalog loads after a restored track starts, so the
                // launch's follow waits for this rebuild in `on_view_installed`.
                this.follow_on_view = true;
                this.refresh_view(cx);
                cx.notify();
                this.refresh_title_bar(cx);
            },
        );
        let (header_compact, header_lines) = fold_head_lines(&config);
        let header_lines_shown = header_lines
            .iter()
            .rposition(|line| !line.is_empty())
            .map_or(1, |last| last + 1);
        let (row_height, head_height) = fold_row_heights(&config);
        let art_margin = fold_margin(config.art_margin, ART_MARGIN_MAX);
        let header_gap_above = fold_margin(config.header_gap_above, HEAD_GAP_MAX);
        let header_gap_below = fold_margin(config.header_gap_below, HEAD_GAP_MAX);
        // An old layout can name a column that sorts on nothing. Drop it, and
        // the next save writes the truth.
        let sort = config
            .sort_key
            .filter(|key| columns::sortable(key))
            .map(|key| (SharedString::from(key), config.sort_desc));
        let labels = columns::label_overrides(&config.column_layout);
        let delegate = TrackTable {
            state: state.clone(),
            panel: cx.weak_entity(),
            view: Arc::new(Vec::new()),
            groups: Vec::new(),
            headers: config.headers,
            group_by: config.group_by,
            group_search_results: config.group_search_results,
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
            // The empty opening view indexes nothing, so any build matches it.
            view_projection: state.library.read(cx).projection_gen(),
            added_now: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            added_now_at: Instant::now(),
            drag_set: None,
        };
        // Click-to-sort turns off the widget's sorting and moves the column drag
        // to Alt, which `set_alt` flips as the modifier comes and goes.
        let table = cx.new(|cx| {
            TableState::new(delegate, window, cx)
                .col_movable(!config.sort_on_click)
                .sortable(!config.sort_on_click)
                .col_selectable(false)
        });
        let _table_events = cx.subscribe_in(&table, window, Self::on_table_event);
        let initial = match config.query_source {
            QuerySource::Global => state.query.read(cx).text().to_string(),
            QuerySource::Local | QuerySource::Selection => config.query.clone(),
        };
        let search =
            cx.new(|cx| SearchBox::new(rox_i18n::t!("query-search"), &initial, window, cx).small());
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        let _query_changed = cx.subscribe(
            &state.query,
            |this: &mut LibraryPanel, _, _: &SharedQueryEvent, cx| {
                this.on_shared_query_changed(cx);
            },
        );
        // Restored as selection-following, open on whatever is picked now.
        let selection_ids = state.selection.read(cx).tracks().to_vec();
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, event: &SelectionEvent, cx| {
                this.on_selection_changed(event.source, cx);
            },
        );
        let _player_changed = cx.observe(&state.player, |this: &mut LibraryPanel, _, cx| {
            this.sync_playing(cx)
        });
        let _thumbs_changed = cx.observe(&state.thumbs, |this: &mut LibraryPanel, _, cx| {
            this.table.update(cx, |_, cx| cx.notify());
        });
        let _portraits_changed = cx.observe(&state.portraits, |this: &mut LibraryPanel, _, cx| {
            this.table.update(cx, |_, cx| cx.notify());
        });
        let focus = cx.focus_handle().tab_stop(true);
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
            group_search_results: config.group_search_results,
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
        // A duplicate opens with a track already playing.
        this.sync_playing(cx);
        this
    }

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

    /// Rescore when the extractor switches or an analysis pass lands.
    /// Edge-triggered: the scan belongs on a change, never on a frame.
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
    /// The raw cosine from `embeddings::scores`, not the tempo-marked ranking
    /// playback draws from, since this column is a look at the vectors.
    ///
    /// Off the UI thread on its own connection. A track change costs about ten
    /// milliseconds on a fifty-thousand-track library. The first question after
    /// the analysis pass writes rereads every vector, a few hundred
    /// milliseconds. Skipped while the column is hidden.
    fn refresh_similarity(&mut self, cx: &mut Context<Self>) {
        if !self.shown_columns(cx).contains("similar") {
            return;
        }
        // The Library page's model is half the score key. Without it, switching
        // extractors leaves the old model's numbers up.
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
            // Nothing playing: drop the scores for the stopped track.
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
                    // Empty means this model hasn't described the corpus yet.
                    // Leave the stamp off so a later pass rescores.
                    delegate.similar_anchor = (!scored.is_empty()).then_some((anchor, model));
                    delegate.similar = Arc::new(scored.into_iter().collect());
                    cx.notify();
                });
                // A similarity-sorted view is ordered by the old scores.
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

    /// Scroll only. The automatic follow never touches the selection.
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

    fn touch_resume(&mut self, cx: &mut Context<Self>) {
        if self.resume_playing {
            self.resume_idle.touch(cx, Self::resume_to_playing);
        }
    }

    /// The clock only fires after a full untouched window, so no idle check.
    fn resume_to_playing(&mut self, cx: &mut Context<Self>) {
        if self.resume_playing {
            self.follow_playing(cx);
        }
    }

    /// Keyboard browsing while the panel itself is focused. In the solo and
    /// popped-out layouts the search toolbar sits inside the panel root, so its
    /// keystrokes bubble through here and get skipped.
    fn on_panel_key(&mut self, event: &KeyDownEvent, window: &Window, cx: &mut Context<Self>) {
        if self.search.read(cx).is_focused(window, cx) {
            return;
        }
        let keystroke = &event.keystroke;
        // The platform chord, so it goes before the modifier bail.
        if keystroke.modifiers.secondary() && keystroke.key.as_str() == "a" {
            self.select_all(cx);
            return;
        }
        if keystroke.modifiers.control || keystroke.modifiers.platform || keystroke.modifiers.alt {
            return;
        }
        self.touch_resume(cx);
        let shift = keystroke.modifiers.shift;
        match keystroke.key.as_str() {
            // A phrase drops first, since it's holding tab, then the selection.
            "escape" => {
                if !self.clear_type_ahead(cx) {
                    self.deselect(cx);
                }
            }
            "up" => self.move_cursor(-1, shift, cx),
            "down" => self.move_cursor(1, shift, cx),
            "pageup" => self.move_cursor(-PAGE_ROWS, shift, cx),
            "pagedown" => self.move_cursor(PAGE_ROWS, shift, cx),
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
                // Space stays play/pause unless a phrase is live.
                if text == " " && !panel::type_ahead_live(self.type_ahead_at) {
                    return;
                }
                // Stop it here, or it also fires the workspace's space-bound
                // TogglePlayback, which this panel inherits unscoped.
                cx.stop_propagation();
                self.type_to(text.clone(), cx);
            }
        }
    }

    /// A grown buffer re-tests the current row first, so refining a match
    /// stays put.
    fn type_to(&mut self, text: String, cx: &mut Context<Self>) {
        let grown = panel::type_ahead_grow(&mut self.type_ahead, &mut self.type_ahead_at, text);
        // A miss still changed the badge, so repaint either way.
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

    /// Hands tab back to Root's panel traversal. True when there was a phrase.
    fn clear_type_ahead(&mut self, cx: &mut Context<Self>) -> bool {
        if self.type_ahead.is_empty() {
            return false;
        }
        self.type_ahead.clear();
        self.type_ahead_at = None;
        cx.notify();
        true
    }

    /// Leaves the window stamp alone, so a run of tabs doesn't revive the badge
    /// or the letter grouping.
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

    /// A `field:` pin shows as the column's label, so `artist:bea` reads
    /// `(Artist) bea`. Empty with no phrase, so the overlay still hides.
    fn type_ahead_display(&self) -> String {
        if let Some((field, needle)) = TrackTable::type_ahead_pin(&self.type_ahead)
            && let Some(column) = columns::columns().iter().find(|c| c.key == field)
        {
            return format!("({}) {}", column.label, needle);
        }
        self.type_ahead.clone()
    }

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

    /// Hands the shared scope back to the whole catalog.
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

    /// With no cursor yet, the first press starts at the edge it heads toward.
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

    fn jump_to_playing(&mut self, cx: &mut Context<Self>) {
        let row = self.table.read(cx).delegate().playing_row;
        if let Some(row) = row {
            self.set_cursor(row, false, cx);
        }
    }

    fn toggle_follow_playing(&mut self, cx: &mut Context<Self>) {
        self.follow_playing = !self.follow_playing;
        if self.follow_playing {
            self.follow_playing(cx);
        }
        cx.notify();
    }

    fn reload_favourites(&mut self, cx: &mut Context<Self>) {
        let favourites = self.state.library.read(cx).favourite_ids();
        self.table.update(cx, |table, cx| {
            table.delegate_mut().favourites = favourites;
            cx.notify();
        });
    }

    /// The pass runs on the background executor: a search over ten million rows
    /// is tens of milliseconds and the sort behind it can be near a second. The
    /// old rows stay up until the new ones land.
    fn refresh_view(&mut self, cx: &mut Context<Self>) {
        self.schedule_view(false, cx);
    }

    /// For keystrokes. The generation check already makes stale passes
    /// harmless, and the wait keeps them from starting at all.
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
            // No projection yet: install the empty view now, not the old rows.
            let live = self.state.library.read(cx).projection_gen();
            self.table.update(cx, |table, cx| {
                install_view(
                    table,
                    generation,
                    live,
                    Arc::new(Vec::new()),
                    Vec::new(),
                    cx,
                );
            });
            self.on_view_installed(cx);
            return;
        };
        let projection_gen = inputs.projection_gen;
        cx.spawn(async move |this, cx| {
            if debounce {
                cx.background_executor().timer(VIEW_DEBOUNCE).await;
                // A newer keystroke landed while this one waited.
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
                    install_view(table, generation, projection_gen, view, groups, cx)
                });
                if installed {
                    this.on_view_installed(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn on_view_installed(&mut self, cx: &mut Context<Self>) {
        // A strict deferred scroll, so it runs on the paint that shows the rows,
        // even in a background tab. The empty initial load keeps it pending.
        if let Some(row) = self.restore_scroll
            && !self.table.read(cx).delegate().view.is_empty()
        {
            self.restore_scroll = None;
            self.table
                .read(cx)
                .vertical_scroll_handle
                .scroll_to_item_strict(row, ScrollStrategy::Top);
        }
        // A refresh that leaves the row in place doesn't re-scroll, so a tag
        // save can't yank the list off whatever was being read.
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
            // Focus moves back to the panel so the playback keys stay with the
            // workspace. The widget also fires this for a double click's clicks.
            TableEvent::SelectRow(ix) => {
                window.focus(&self.focus);
                let ix = *ix;
                // A header selects its group whole, and the modifiers work by
                // group the way they do by row. Disc dividers just clear.
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
                        let range: Vec<usize> = (lo..=hi)
                            .filter(|&i| delegate.track_at(i).is_some())
                            .collect();
                        // Ctrl+Shift stacks a second block on, plain shift replaces.
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
                            // The widget put its focus row here too.
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
            // A header plays its group whole. A disc divider's rows come back
            // empty.
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
            // The widget already reordered the delegate's columns.
            TableEvent::MoveColumn(..) => self.request_layout_save(cx),
            _ => {}
        }
    }

    fn browse(&mut self, cx: &mut Context<Self>) {
        crate::catalog::browse(&self.state.library, cx);
    }

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
            group_search_results: self.group_search_results,
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

    /// The view row at the top of the viewport. The list never reports child
    /// bounds to its base handle (its `last_item_size.item` is the viewport),
    /// so this walks the pixel offset over the rows' own heights. A pending
    /// restore reports its target, so an unshown panel keeps its position.
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
        // A dump save runs outside the render, so the thread-local panel scale
        // isn't set. Read the override off the theme instead.
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
        // Rows aren't uniform. Layout dumps only, never a paint, so the loop is
        // fine.
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

    fn toggle_column(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let Some(def) = column_def(key) else { return };
        let mut sort_cleared = false;
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            if let Some(ix) = delegate.columns.iter().position(|c| c.key.as_ref() == key) {
                // Never drop the last column: an empty table has no header
                // to bring one back from.
                if delegate.columns.len() > 1 {
                    delegate.columns.remove(ix);
                    // A hidden sort column can't clear its sort.
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
                let label: SharedString = match delegate.labels.get(def.key) {
                    Some(label) => label.clone().into(),
                    None => def.label.into(),
                };
                let column = Column::new(def.key, label).width(px(def.default_width));
                // Same gate as the restored layout, or the column stops
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
        // Turning Similar on is the first ask for a score.
        self.refresh_similarity(cx);
        self.refresh_title_bar(cx);
        self.request_layout_save(cx);
    }

    /// None restores the registry's label. An empty name draws the header blank
    /// and persists as one.
    fn set_column_label(&mut self, key: String, label: Option<String>, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            match label.clone() {
                Some(label) => delegate.labels.insert(key.clone(), label),
                None => delegate.labels.remove(&key),
            };
            // A hidden column's name still lands in the map for when it's back.
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

    fn column_label(&self, key: &str, cx: &App) -> Option<String> {
        self.table.read(cx).delegate().labels.get(key).cloned()
    }

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

    fn set_sort_on_click(&mut self, on: bool, cx: &mut Context<Self>) {
        self.sort_on_click = on;
        self.table.update(cx, |table, cx| {
            table.sortable = !on;
            // On, the drag waits for Alt, which nobody holds as this flips.
            table.col_movable = !on;
            table.delegate_mut().sort_on_click = on;
            // The widget's column copies never saw the sorts taken while it
            // was off. Re-reading puts its arrows back on the sorted column.
            table.refresh(cx);
        });
        cx.notify();
        self.request_layout_save(cx);
    }

    /// gpui can't gate `on_drag` on a modifier, so with click-to-sort on the
    /// column drag only arms while Alt is held.
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

    fn shown_columns(&self, cx: &App) -> HashSet<String> {
        self.table
            .read(cx)
            .delegate()
            .columns
            .iter()
            .map(|c| c.key.to_string())
            .collect()
    }

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

    fn reset_columns(&mut self, cx: &mut Context<Self>) {
        let sort = self.table.read(cx).delegate().sort.clone();
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            // Renames survive. Each renamed header has its own reset.
            delegate.columns = track_columns(&[], &sort, &delegate.labels);
            table.refresh(cx);
        });
        self.columns_shown = self.shown_columns(cx);
        self.refresh_title_bar(cx);
        self.request_layout_save(cx);
    }

    /// The dock never sees column changes, so bounce a LayoutChanged through
    /// the host tab panel for the workspace's debounced save. Without it a
    /// relaunch can lose them.
    fn request_layout_save(&self, cx: &mut Context<Self>) {
        if let Some(tabs) = self.tab_panel.as_ref().and_then(|w| w.upgrade()) {
            tabs.update(cx, |_, cx| cx.emit(PanelEvent::LayoutChanged));
        }
    }

    /// Docked, the controls live in the tab panel's title bar, which only
    /// repaints when the tab panel is notified. Call after anything it shows.
    fn refresh_title_bar(&self, cx: &mut App) {
        if let Some(tabs) = self.tab_panel.as_ref().and_then(|tabs| tabs.upgrade()) {
            tabs.update(cx, |_, cx| cx.notify());
        }
    }

    /// Play a bounded window of the view around the clicked track, so Prev steps
    /// back and Next runs on. This is the playing context, never the queue.
    fn play_from(&mut self, ix: usize, cx: &mut Context<Self>) {
        // The engine pins the head, so the clicked track still plays first.
        if self.state.player.read(cx).shuffle() {
            self.play_shuffled_from(ix, cx);
            return;
        }
        let window = {
            let view = self.table.read(cx).delegate().view.clone();
            play_window(&view, ix, QUEUE_CAP)
        };
        let Some((rows, start)) = window else { return };
        self.play_rows_at(rows, start, cx);
    }

    /// Context like a library run, so the queue keeps what was hand-picked
    /// (ADR 16).
    fn play_rows(&mut self, rows: Vec<usize>, cx: &mut Context<Self>) {
        self.play_rows_at(rows, 0, cx);
    }

    fn play_rows_at(&mut self, rows: Vec<usize>, start: usize, cx: &mut Context<Self>) {
        let (result, scope) = {
            let delegate = self.table.read(cx).delegate();
            let view = delegate.view.clone();
            let Some(projection) = delegate.projection(cx) else {
                return;
            };
            let ids: Vec<i64> = rows
                .into_iter()
                .filter_map(|ix| match view.get(ix) {
                    Some(&Row::Track(row)) => Some(projection.db_id[row as usize]),
                    _ => None,
                })
                .collect();
            // The whole view, not the queued window: continuation runs on into
            // the rows below it (ADR 17).
            let order: Vec<i64> = view
                .iter()
                .filter_map(|row| match row {
                    &Row::Track(row) => Some(projection.db_id[row as usize]),
                    _ => None,
                })
                .collect();
            (
                self.state.library.read(cx).keys_for(&ids),
                continuation::Scope::View(order.into()),
            )
        };
        match result {
            Ok(keys) => self.state.player.update(cx, |player, cx| {
                player.play_at(keys, start, cx);
                // After the play: starting a session resets the scope.
                player.set_scope(scope);
            }),
            Err(e) => {
                self.error = Some(format!("library: {e}").into());
                cx.notify();
                self.refresh_title_bar(cx);
            }
        }
    }

    /// The clicked track itself doesn't play. A double click already does.
    fn play_similar(&mut self, row_ix: usize, cx: &mut Context<Self>) {
        let Some(row) = self.table.read(cx).delegate().track_at(row_ix) else {
            return;
        };
        let Some(&id) = self
            .table
            .read(cx)
            .delegate()
            .projection(cx)
            .and_then(|projection| projection.db_id.get(row as usize))
        else {
            return;
        };
        let library = self.state.library.clone();
        self.state
            .player
            .update(cx, |player, cx| player.play_similar_to(id, &library, cx));
    }

    /// The engine pins the head when shuffle engages, so the first row plays
    /// first. Rows past the cap drop.
    fn play_shuffled(&mut self, mut rows: Vec<usize>, cx: &mut Context<Self>) {
        rows.truncate(QUEUE_CAP);
        self.state
            .player
            .update(cx, |player, _| player.set_shuffle(true));
        self.play_rows_at(rows, 0, cx);
    }

    /// Continuation draws the rest of the view and then the library behind the
    /// seed (ADR 17), so the seed only has to be enough to start on.
    fn play_shuffled_from(&mut self, ix: usize, cx: &mut Context<Self>) {
        let rows = shuffle_seed(&self.table.read(cx).delegate().view, ix, SHUFFLE_SEED);
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
            SearchEvent::FocusChanged => {
                cx.notify();
                self.refresh_title_bar(cx);
            }
            // Leaving the box hands the playback keys back to the workspace.
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

    /// The popped-out window has no title bar, so the controls sit in a toolbar
    /// row. Catalog status lives in the workspace menubar.
    fn toolbar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
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

    fn track_list(&self) -> impl IntoElement + use<> {
        Table::new(&self.table)
            .stripe(self.stripes)
            .row_borders(self.row_borders)
            .header_visible(self.column_headers)
            .bordered(false)
            // The rows draw their own selection wash. The widget's overlay would
            // swap the bottom hairline for a ring the row clip eats (vendor patch).
            .row_selection_style(false)
            .row_spacing(px(self.row_spacing))
            // A custom size is the row height itself (vendor patch).
            .with_size(Size::Size(px(self.row_height)))
    }

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

    /// Pure paint: the block rows size to the line height, not the text.
    fn set_head_text(&mut self, size: f32, cx: &mut Context<Self>) {
        if self.head_text == size {
            return;
        }
        self.head_text = size;
        self.table
            .update(cx, |table, _| table.delegate_mut().head_text = size);
        cx.notify();
    }

    /// The view's rows don't move, so no rebuild and the selection stays.
    fn set_head_height(&mut self, height: f32, cx: &mut Context<Self>) {
        if self.head_height == height {
            return;
        }
        self.head_height = height;
        self.table
            .update(cx, |table, _| table.delegate_mut().head_height = height);
        cx.notify();
    }

    fn set_header_gap_above(&mut self, gap: f32, cx: &mut Context<Self>) {
        if self.header_gap_above == gap {
            return;
        }
        self.header_gap_above = gap;
        self.table
            .update(cx, |table, _| table.delegate_mut().header_gap_above = gap);
        cx.notify();
    }

    fn set_header_gap_below(&mut self, gap: f32, cx: &mut Context<Self>) {
        if self.header_gap_below == gap {
            return;
        }
        self.header_gap_below = gap;
        self.table
            .update(cx, |table, _| table.delegate_mut().header_gap_below = gap);
        cx.notify();
    }

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

    /// The editor sends every open well, empties included, so the open count
    /// follows its adds and removes. Slots past it clear.
    fn set_head_lines(&mut self, rows: Vec<Vec<HeadPiece>>, cx: &mut Context<Self>) {
        self.header_lines_shown = rows.len().clamp(1, HEAD_LINE_SLOTS);
        for slot in 0..HEAD_LINE_SLOTS {
            if let Some(line) = self.header_lines.get_mut(slot) {
                *line = rows.get(slot).cloned().unwrap_or_default();
            }
        }
        self.sync_head_lines(cx);
    }

    fn set_head_compact(&mut self, items: Vec<HeadPiece>, cx: &mut Context<Self>) {
        self.header_compact = items;
        self.sync_head_lines(cx);
    }

    fn sync_head_lines(&mut self, cx: &mut Context<Self>) {
        let lines = effective_head_lines(self.headers, &self.header_compact, &self.header_lines);
        self.table
            .update(cx, |table, _| table.delegate_mut().head_lines = lines);
        self.refresh_view(cx);
        cx.notify();
    }

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

    /// Presentation only. Off lists search results flat.
    fn set_group_search_results(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.group_search_results == on {
            return;
        }
        self.group_search_results = on;
        self.table.update(cx, |table, _| {
            table.delegate_mut().group_search_results = on;
        });
        self.refresh_view(cx);
        cx.notify();
    }

    /// The look knobs stay on Appearance and the column checklist on View.
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

    fn empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        div()
            .id("library-empty")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(tokens::SPACE_SM)
            .p(tokens::SPACE_MD)
            .text_center()
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
                        // The box keeps its text. The view shows the full catalog
                        // while hidden.
                        this.refresh_view(cx);
                        cx.notify();
                        this.refresh_title_bar(cx);
                    },
                    self.query_source,
                    |this: &mut Self, source, cx| this.pick_query_source(source, cx),
                    cx,
                ))
                .when(self.headers != Headers::Off, |d| {
                    d.child(panel::setting_row(
                        rox_i18n::t!("library-group-search-results"),
                        Some(rox_i18n::t!("library-group-search-results.description")),
                        panel::toggle(
                            self.group_search_results,
                            |this: &mut Self, on, cx| this.set_group_search_results(on, cx),
                            cx,
                        ),
                    ))
                })
                .child(panel::tracking_section(
                    self.follow_playing,
                    rox_i18n::t!("library-follow-description"),
                    |this: &mut Self, on, cx| {
                        this.follow_playing = on;
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

    /// Stored on the config because these shape the content, not the panel
    /// frame.
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
                .when(header_mode != Headers::Off, |d| {
                    d.child(settings_ui::section(
                        rox_i18n::t!("library-headers"),
                        None,
                        headers,
                    ))
                })
                // Always shown, so switching the grouping never moves an art knob.
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
    /// Debounced, since the costly query changes are keystrokes.
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

    /// Kept compact: the title row is 30px.
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

    /// The table serves row context menus over the whole body.
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

    /// Read back by the builder in `workspace::register_panels`.
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
        // Jump and Follow first, then the view knobs in flyouts so the menu
        // stays short.
        let weak = cx.entity().downgrade();
        let weak_f = cx.entity().downgrade();
        let follow = self.follow_playing;
        // Checks on the right, or they'd replace these two items' icons.
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

        // The flyouts build off the panel's copies, never the table: this menu
        // also builds inside the row context menu, mid-table-update.
        let menu = menu.separator().label(rox_i18n::t!("library-menu-display"));

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

        let menu = crate::query::shared_query::search_flyout(
            menu,
            |this: &Self| this.query_source,
            |this: &Self| this.show_search,
            &cx.entity(),
            |this: &mut Self, source, cx| this.pick_query_source(source, cx),
            |this: &mut Self, on, cx| {
                this.show_search = on;
                this.refresh_view(cx);
                cx.notify();
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
        if self.resync_box {
            self.resync_box = false;
            self.sync_query_box(window, cx);
        }
        self.watch_similarity(cx);
        // The follow glide steps here once per frame until it arrives.
        let dt = self.glide_tick.elapsed().as_secs_f32().min(0.05);
        self.glide_tick = Instant::now();
        if let Some(row) = self.glide_to {
            let (handle, target, in_view) = {
                let table = self.table.read(cx);
                // Header rows size to their content, so take the table's cached
                // row offset rather than a uniform-stride estimate.
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
                // A view swap can strand the target past the end.
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
        // Keys off the loaded projection, never the view. Off the view the call
        // to action would flash during load and show when a search hides every row.
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
        // Sharing a group, the controls sit in the tab bar via title_suffix.
        // Solo or popped out there's no header, so the toolbar renders here.
        let headerless = self
            .tab_panel
            .as_ref()
            .and_then(|tabs| tabs.upgrade())
            .is_none_or(|tabs| tabs.read(cx).panels_count() < 2);
        // The root must size itself: the dock lays the panel out as a cached
        // absolute root, where flex_1 has no flex parent to grow in.
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_panel())
            .track_focus(&self.focus)
            // Bindings win over key listeners, so a live phrase carries contexts
            // that scope out the workspace's space binding and Root's tab traversal.
            .when_some(
                panel::type_ahead_context(&self.type_ahead, self.type_ahead_at),
                |d, context| d.key_context(context),
            )
            // A press anywhere ends the phrase. Capture phase, so rows that stop
            // the press can't hide it.
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                this.clear_type_ahead(cx);
            }))
            .on_action(cx.listener(|this, _: &TypeAheadNext, _, cx| this.type_step(false, cx)))
            .on_action(cx.listener(|this, _: &TypeAheadPrev, _, cx| this.type_step(true, cx)))
            .on_key_down(
                cx.listener(|this, event, window, cx| this.on_panel_key(event, window, cx)),
            )
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, cx| {
                this.set_alt(event.modifiers.alt, cx);
            }))
            // These only restart the idle clock and leave the event to the
            // table, so nothing acts twice.
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

/// Edits apply as they're typed, so Enter or Escape just closes.
struct ColumnRenameWindow {
    panel: WeakEntity<LibraryPanel>,
    input: Entity<InputState>,
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
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .default_value(current)
        });
        // Held by key: columns can be reordered or hidden while this is open.
        let _input_events = cx.subscribe_in(
            &input,
            window,
            move |this: &mut Self, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    // Empty restores the registry's label. A lone space trims
                    // to an empty name, which draws the header blank.
                    let raw = input.read(cx).value().to_string();
                    let label = (!raw.is_empty()).then(|| raw.trim().to_string());
                    this.panel
                        .update(cx, |panel, cx| {
                            panel.set_column_label(key.clone(), label, cx)
                        })
                        .ok();
                }
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
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, window, _| {
                if event.keystroke.key == "escape" {
                    window.remove_window();
                }
            }))
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
    use rox_library::{TrackRow, store};

    fn track(path: &str, album_artist: &str, album: &str, track_no: u16) -> TrackRow {
        TrackRow {
            remote_url: String::new(),
            remote_live: false,
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

    fn projection(rows: &[TrackRow]) -> Arc<Projection> {
        let mut conn = rox_library::rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, rows).unwrap();
        Arc::new(Projection::load_serial(&conn, false).unwrap())
    }

    /// The answer [`play_window`] has to give without listing every track.
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

    #[test]
    fn the_shuffle_seed_samples_the_whole_view() {
        let view = view_with_heads(400, 10);
        let ix = (0..view.len())
            .find(|&i| matches!(view[i], Row::Track(_)))
            .expect("a track row");
        let mut landed = std::collections::HashSet::new();
        for _ in 0..20 {
            let rows = shuffle_seed(&view, ix, 30);
            assert_eq!(rows[0], ix);
            assert_eq!(rows.len(), 31);
            let distinct: std::collections::HashSet<usize> = rows.iter().copied().collect();
            assert_eq!(distinct.len(), rows.len(), "a row came back twice");
            assert!(rows.iter().all(|&i| matches!(view[i], Row::Track(_))));
            landed.extend(rows.iter().copied());
        }
        assert!(
            landed.iter().any(|&i| i > view.len() / 2),
            "the draw never left the top of the view"
        );
        // A header row pins nothing; a small view comes back whole.
        assert!(!shuffle_seed(&view, 0, 5).contains(&0));
        let small = view_with_heads(4, 2);
        assert_eq!(shuffle_seed(&small, 1, 100).len(), 4);
    }

    #[test]
    fn the_play_window_needs_a_track_row() {
        let view = view_with_heads(4, 2);
        assert!(play_window(&view, 0, 10).is_none());
        assert!(play_window(&view, view.len(), 10).is_none());
    }

    #[test]
    fn the_play_window_fills_the_budget() {
        let view = view_with_heads(100, 4);
        for ix in (0..view.len()).filter(|&i| matches!(view[i], Row::Track(_))) {
            let (rows, start) = play_window(&view, ix, 20).expect("a track row");
            assert_eq!(rows.len(), 20);
            assert_eq!(rows[start], ix);
        }
    }

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
                grouping: inputs
                    .head_rows
                    .filter(|_| inputs.query.is_empty() || inputs.group_search_results)
                    .map(|head_rows| Grouping {
                        head_rows,
                        pre_sort: if inputs.query.is_empty() {
                            None
                        } else {
                            Some(GroupBy::Album.search_sort())
                        },
                        key: &key,
                        discs: true,
                    }),
            },
        )
    }

    fn inputs(projection: &Arc<Projection>, query: &str, head_rows: Option<u8>) -> ViewInputs {
        ViewInputs {
            projection: projection.clone(),
            projection_gen: 0,
            order: Arc::new(projection.sort_canonical()),
            query: query.to_string(),
            filter: FilterSet::default(),
            similar: None,
            sort: None,
            group_by: GroupBy::Album,
            group_search_results: true,
            head_rows,
        }
    }

    #[test]
    fn search_grouping_setting_controls_headers() {
        let p = projection(&[
            track("/m/a1.flac", "A", "One", 1),
            track("/m/a2.flac", "A", "One", 2),
            track("/m/b1.flac", "B", "Two", 1),
        ]);
        let mut grouped = inputs(&p, "a1", Some(1));
        grouped.group_search_results = true;
        let (rows, groups) = compute_rows(&grouped);
        assert_eq!(groups.len(), 1);
        assert_eq!(rows.len(), 2);
        assert!(matches!(rows[0], Row::Head(0, 0)));
        assert!(matches!(rows[1], Row::Track(_)));

        let mut flat = inputs(&p, "a1", Some(1));
        flat.group_search_results = false;
        let (rows, groups) = compute_rows(&flat);
        assert!(groups.is_empty());
        assert_eq!(rows.len(), 1);
        assert!(matches!(rows[0], Row::Track(_)));
    }

    #[test]
    fn grouped_album_search_restores_canonical_order() {
        let p = projection(&[
            track("/m/a2.flac", "A", "Taking You Higher", 2),
            track("/m/other.flac", "B", "Another Album", 1),
            track("/m/a1.flac", "A", "Taking You Higher", 1),
        ]);
        let grouped = inputs(&p, "Taking You Higher", Some(1));
        let (rows, groups) = compute_rows(&grouped);

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].tracks, 2);
        assert!(matches!(rows.first(), Some(Row::Head(0, 0))));
        let track_nos: Vec<u16> = rows
            .iter()
            .filter_map(|row| match row {
                Row::Track(row) => Some(p.track_no[*row as usize]),
                _ => None,
            })
            .collect();
        assert_eq!(track_nos, vec![1, 2]);
    }

    /// Running on a plain thread also pins the inputs as `Send`.
    #[test]
    fn the_background_pass_computes_the_same_view() {
        let p = projection(&[
            track("/m/a1.flac", "A", "One", 1),
            track("/m/a2.flac", "A", "One", 2),
            track("/m/b1.flac", "B", "Two", 1),
            track("/m/b2.flac", "B", "Two", 2),
            track("/m/c1.flac", "C", "Three", 1),
        ]);
        for (query, head_rows, group_search_results) in [
            ("", Some(2u8), true),
            ("", None, true),
            ("b", Some(2u8), true),
            ("b", Some(2u8), false),
        ] {
            let mut direct_inputs = inputs(&p, query, head_rows);
            direct_inputs.group_search_results = group_search_results;
            let direct = view_directly(&direct_inputs);
            let mut sent = inputs(&p, query, head_rows);
            sent.group_search_results = group_search_results;
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
