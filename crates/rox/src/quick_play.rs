//! The quick-play modal: Ctrl/Cmd+P or Ctrl/Cmd+F drops a search box over
//! the workspace to jump straight to a track. Typing filters the whole
//! catalog through the projection's search, enter or a click queues from
//! the picked track in result order, escape closes. A view over the same
//! shared catalog and player the panels use, hosted as an overlay instead
//! of a dock item; the workspace owns one at most and drops it on dismiss.
//!
//! The Power Search window hosts the same view (see
//! [`set_hosted`](QuickPlay::set_hosted)), and there it's a place to work
//! rather than a jump: a click picks rows the way a track list does, a
//! double click or enter plays, a right click opens the shared track
//! actions, and playing leaves the window up so the next pick is one
//! click away. Escape is what closes it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    div, prelude::*, px, relative, svg, uniform_list, Action, App, Context, DismissEvent, Div,
    Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Modifiers, MouseButton,
    MouseDownEvent, ScrollStrategy, SharedString, Subscription, UniformListScrollHandle, Window,
};
use gpui_component::input::{MoveDown, MovePageDown, MovePageUp, MoveUp, SelectAll};
use gpui_component::menu::ContextMenuExt;
use rox_core::fmt::fmt_ms;
use rox_core::QUEUE_CAP;
use rox_library::projection::{FilterSet, Projection, QUERY_FIELDS};
use rox_playback::engine::shuffle_slice;

use rox_core::settings::{QuickPlayConfig, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState};
use rox_panel_api::query::search::{SearchBox, SearchEvent};
use rox_panel_api::suggest;
use rox_panel_api::track_ui::track_columns;
use rox_services::catalog::LibraryEvent;
use rox_services::thumbs::Thumb;

/// One result row's height; the list is a uniform_list, so every row must
/// agree on it. Comfortable rows run taller.
const ROW_H: f32 = 30.;
const ROW_H_COMFORTABLE: f32 = 40.;

/// What the subtitle line under the title adds to a row's height.
const SUBTITLE_H: f32 = 14.;

/// How many rows show before the list scrolls.
const VISIBLE_ROWS: usize = 14;

/// How many artist and album entries a search surfaces above the track
/// hits, so a broad query can't bury the tracks under group rows.
const MAX_ARTIST_HITS: usize = 6;
const MAX_ALBUM_HITS: usize = 8;

/// How far page up and page down step, most of a full view.
const PAGE_ROWS: isize = 10;

/// The fields a bare `-field` term can ask for the absence of; the rest
/// of the query fields always carry a value, so there'd be nothing to
/// find. Drives the footer's "Without" chips.
const ABSENCE_FIELDS: &[&str] = &[
    "year",
    "genre",
    "artist",
    "albumartist",
    "album",
    "title",
    "rating",
    "plays",
];

/// The syntax sheet's lines, in the order it lists them. Each key holds
/// the plain description, its `.example` attribute the query fragment a
/// click appends.
const SYNTAX_KEYS: &[&str] = &[
    "quick-play-syntax-free",
    "quick-play-syntax-field",
    "quick-play-syntax-numeric",
    "quick-play-syntax-year",
    "quick-play-syntax-absent",
    "quick-play-syntax-exclude",
];

/// A group entry the search surfaces above the track hits: a whole artist
/// or album, holding the interned symbols to gather its tracks and a
/// representative projection row for the cover. Playing one queues every
/// track it holds instead of a single file.
#[derive(Clone, Copy)]
enum Head {
    Artist {
        album_artist: u32,
        row: u32,
    },
    Album {
        album_artist: u32,
        album: u32,
        row: u32,
    },
}

/// One resolved list row, ready to render: a track or a group head, already
/// pulled through the projection so the render pass touches no library.
struct RowInfo {
    /// The combined-list index, for selection and the play click.
    ix: usize,
    title: SharedString,
    /// The title's sort name, drawn after it as a reading. The subtitle
    /// gets none: it's a composed "artist - album" line, and two readings
    /// inside one would read worse than none.
    title_reading: SharedString,
    sub: SharedString,
    /// The right-hand cell: a track's duration (blank when unknown) or a
    /// head's kind tag ("Album"/"Artist").
    trailing: SharedString,
    path: Option<PathBuf>,
    /// A head shows its tag whatever the toggles; a track's duration cell
    /// follows the show-duration switch and drops when blank.
    is_head: bool,
}

impl Head {
    /// The representative projection row, for the cover thumbnail.
    fn row(&self) -> u32 {
        match *self {
            Head::Artist { row, .. } | Head::Album { row, .. } => row,
        }
    }

    /// Whether a projection row belongs to this group, the play gather's
    /// per-row test over the browse order.
    fn contains(&self, projection: &Projection, row: u32) -> bool {
        let i = row as usize;
        match *self {
            Head::Artist { album_artist, .. } => projection.album_artist[i] == album_artist,
            Head::Album {
                album_artist,
                album,
                ..
            } => projection.album_artist[i] == album_artist && projection.album[i] == album,
        }
    }
}

/// An explicit set of tracks the search runs inside, and the name it goes by.
#[derive(Clone)]
pub struct Seed {
    pub ids: Vec<i64>,
    pub label: SharedString,
}

/// A seed resolved against the current projection: the row mask its ids
/// come out as, plus the artist and album groups those rows cover so a
/// head can be tested with a hash lookup instead of a scan each.
struct SeedRows {
    mask: Vec<bool>,
    artists: HashSet<u32>,
    albums: HashSet<(u32, u32)>,
}

/// Resolve a seed's ids into a row mask and the groups its rows sit in.
/// One `filter_mask` and one pass over it, so a refresh pays for the
/// seed once instead of once per row or per head. Building the groups
/// here is what lets a head be kept on whether any of its tracks is in
/// the seed rather than on its representative row alone, which would
/// drop an album the seed holds most of just because the row the search
/// picked out for the cover isn't one of them.
fn seed_rows(projection: &Projection, ids: &[i64]) -> SeedRows {
    let mask = projection
        .filter_mask(&FilterSet::with_ids(ids.to_vec()))
        .unwrap_or_else(|| vec![false; projection.len()]);
    let mut artists = HashSet::new();
    let mut albums = HashSet::new();
    for (row, ok) in mask.iter().enumerate() {
        if !ok {
            continue;
        }
        artists.insert(projection.album_artist[row]);
        albums.insert((projection.album_artist[row], projection.album[row]));
    }
    SeedRows {
        mask,
        artists,
        albums,
    }
}

impl SeedRows {
    /// Whether a projection row is in the seed. A mask built over an
    /// older projection can be shorter than the current one; a row past
    /// its end is treated as outside, so a stale mask narrows rather
    /// than indexes out of bounds.
    fn keeps(&self, row: u32) -> bool {
        self.mask.get(row as usize).copied().unwrap_or(false)
    }

    /// Whether any of a head's tracks is in the seed.
    fn keeps_head(&self, head: Head) -> bool {
        match head {
            Head::Artist { album_artist, .. } => self.artists.contains(&album_artist),
            Head::Album {
                album_artist,
                album,
                ..
            } => self.albums.contains(&(album_artist, album)),
        }
    }
}

/// What a click leaves highlighted, given what's held down: shift takes
/// the run from the anchor to the row, ctrl or cmd toggles the row on its
/// own, both together stack the run onto what's already lit so a second
/// block can join the first, a plain click takes it alone. Returns the
/// new set and the anchor it leaves behind, so a shift click keeps
/// measuring from where the last plain one landed. Pure over indices, no
/// window or projection needed.
fn click_selection(
    selected: &HashSet<usize>,
    anchor: Option<usize>,
    ix: usize,
    shift: bool,
    toggle: bool,
) -> (HashSet<usize>, Option<usize>) {
    if shift {
        let start = anchor.unwrap_or(ix);
        let (lo, hi) = (start.min(ix), start.max(ix));
        let range = lo..=hi;
        let set = if toggle {
            selected.iter().copied().chain(range).collect()
        } else {
            range.collect()
        };
        (set, Some(start))
    } else if toggle {
        let mut set = selected.clone();
        if !set.remove(&ix) {
            set.insert(ix);
        }
        (set, Some(ix))
    } else {
        (HashSet::from([ix]), Some(ix))
    }
}

/// The lit row closest to `from`, where `from` itself isn't lit. Ties go
/// to the row above, the one the eye is already on after a click took the
/// row under the pointer out of the set.
fn nearest_selected(selected: &HashSet<usize>, from: usize) -> Option<usize> {
    selected
        .iter()
        .copied()
        .min_by_key(|&ix| (ix.abs_diff(from), ix))
}

pub struct QuickPlay {
    state: AppState,
    /// The query editor, the shared search box; `query` copies its value
    /// via change events.
    search: Entity<SearchBox>,
    query: String,
    /// The album and artist entries a search surfaces above the tracks:
    /// typing a name jumps to the whole album or artist, not just its
    /// tracks. Empty while the query is, so browsing is tracks only.
    heads: Vec<Head>,
    /// Projection rows matching the query: a shuffle of the whole library
    /// while the query is empty, search order otherwise. The list shows
    /// [`heads`](Self::heads) first, then these.
    hits: Arc<Vec<u32>>,
    /// The highlighted rows, indexes into the combined list, heads first.
    /// As an overlay it only ever holds the cursor; hosted, a click can
    /// grow it.
    selected: HashSet<usize>,
    /// The keyboard cursor, what enter plays and what the arrows move.
    /// Always in [`selected`](Self::selected).
    cursor: usize,
    /// Where a shift click measures its range from, the last row taken
    /// without shift.
    anchor: Option<usize>,
    /// The row a right press landed on, read when the context menu builds
    /// a frame later. Cleared by any right press that misses a row.
    menu_row: Option<usize>,
    /// Cover path per db id, filled on a row's first paint and reused after,
    /// so a scroll through a big result list doesn't run a store query per
    /// visible row every frame. Cleared whenever the hits rebuild.
    cover_paths: HashMap<i64, Option<PathBuf>>,
    scroll: UniformListScrollHandle,
    /// A failed play, shown until the next query change.
    error: Option<SharedString>,
    /// The result list's appearance knobs, copied from settings and
    /// written back on every edit.
    config: QuickPlayConfig,
    /// Whether the inline config panel is open, beside the search box.
    show_config: bool,
    /// The set the search runs inside, when a host seeded one; None is
    /// the whole library, the overlay's own case.
    seed: Option<Seed>,
    /// [`seed`](Self::seed) resolved against the projection, rebuilt on
    /// every refresh and read again by the play gather.
    seed_rows: Option<SeedRows>,
    /// Whether the syntax sheet is open, above the footer. Never
    /// persisted: it's a reminder, not a preference.
    show_syntax: bool,
    /// Whether a window hosts this view instead of the workspace overlay.
    /// The layout, the click handling, and what a play does all read it:
    /// see [`set_hosted`](Self::set_hosted).
    hosted: bool,
    _input_events: Subscription,
    _library_changed: Subscription,
    _thumbs_changed: Subscription,
}

impl EventEmitter<DismissEvent> for QuickPlay {}

impl Focusable for QuickPlay {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.search.read(cx).focus_handle(cx)
    }
}

impl QuickPlay {
    pub fn new(state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            SearchBox::new(
                rox_i18n::t!("quick-play-search-placeholder"),
                "",
                window,
                cx,
            )
        });
        let _input_events = cx.subscribe_in(&search, window, Self::on_search_event);
        // A scan finishing mid-search would leave the hits pointing into
        // the old projection; recompute over the new one, suggestions
        // included.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut QuickPlay, _, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.attach_suggestions(cx);
                this.refresh(cx);
            },
        );
        // A cover that finishes loading repaints the result rows.
        let _thumbs_changed = cx.observe(&state.thumbs, |_: &mut QuickPlay, _, cx| cx.notify());
        let mut this = QuickPlay {
            state,
            search,
            query: String::new(),
            heads: Vec::new(),
            hits: Arc::new(Vec::new()),
            selected: HashSet::from([0]),
            cursor: 0,
            anchor: Some(0),
            menu_row: None,
            cover_paths: HashMap::new(),
            scroll: UniformListScrollHandle::new(),
            error: None,
            config: Settings::load().look.bundle.appearance.quick_play,
            show_config: false,
            seed: None,
            seed_rows: None,
            show_syntax: false,
            hosted: false,
            _input_events,
            _library_changed,
            _thumbs_changed,
        };
        this.attach_suggestions(cx);
        this.refresh(cx);
        this
    }

    /// Turn the view into the Power Search window's content. The overlay
    /// leaves this off, and everything it changes is a difference between
    /// a modal you pass through and a window you work in:
    ///
    /// - The view fills the frame instead of sizing to its rows. Over the
    ///   workspace the modal is a card, and a card as tall as the app
    ///   would cover what the search is being run against.
    /// - A click selects instead of playing, with ctrl/cmd and shift
    ///   growing the set, and a double click or enter plays. The overlay
    ///   plays on the first click, which is the whole point of it.
    /// - A right click opens the track actions every song surface shares.
    /// - Playing doesn't dismiss, so the results stay up for the next
    ///   pick. Escape is what closes the window.
    pub fn set_hosted(&mut self, hosted: bool) {
        self.hosted = hosted;
    }

    /// Point the search box's suggestion menu at the current projection;
    /// at open and again whenever a scan produces a new one.
    fn attach_suggestions(&self, cx: &mut Context<Self>) {
        let provider = suggest::query_provider(&self.state.library, cx);
        self.search
            .update(cx, |search, cx| search.set_completions(provider, cx));
    }

    /// Run the search inside an explicit set of tracks, or over the whole
    /// library again with None. The chip under the search box names the
    /// set; the results, heads included, stay inside it until it's
    /// dropped.
    pub fn set_seed(&mut self, seed: Option<Seed>, cx: &mut Context<Self>) {
        self.seed = seed;
        self.refresh(cx);
    }

    /// Put text in the search box and search it, the path a host takes to
    /// hand the modal a query it didn't type (opening it already narrowed).
    pub fn set_query(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.search
            .update(cx, |search, cx| search.set_value(text, window, cx));
        self.query = text.to_string();
        self.refresh(cx);
    }

    /// Each result row's height, taller when comfortable rows are on and
    /// again when the subtitle line shows. Scaled by the app font like the
    /// track-list panels, so the modal's rows track the text and the list
    /// height derived from this stays in step.
    fn row_h(&self) -> f32 {
        let base = if self.config.comfortable {
            ROW_H_COMFORTABLE
        } else {
            ROW_H
        };
        let base = if self.config.show_subtitle {
            base + SUBTITLE_H
        } else {
            base
        };
        base * palette::font_scale()
    }

    /// Let the search box's suggestion menu take an action first; true
    /// when it was open and consumed it.
    fn menu_action(
        &mut self,
        action: Box<dyn Action>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.search
            .update(cx, |search, cx| search.menu_action(action, window, cx))
    }

    /// Flip the config panel open or shut.
    fn toggle_config(&mut self, cx: &mut Context<Self>) {
        self.show_config = !self.show_config;
        cx.notify();
    }

    /// Change one config knob, persist it, repaint.
    fn edit_config(&mut self, edit: impl FnOnce(&mut QuickPlayConfig), cx: &mut Context<Self>) {
        edit(&mut self.config);
        let config = self.config.clone();
        Settings::update(move |s| s.look.bundle.appearance.quick_play = config);
        cx.notify();
    }

    /// The combined result count: the album and artist entries, then the
    /// track hits. What selection and the list index against.
    fn len(&self) -> usize {
        self.heads.len() + self.hits.len()
    }

    /// Recompute the hits for the current query and reset the highlight
    /// to the top. A non-empty query also gathers the matching artists and
    /// albums to show above the tracks; browsing stays tracks only, drawn
    /// at random so the modal isn't the same fourteen tracks every time.
    /// A seed narrows all of it to its own rows and lists them in browse
    /// order while nothing's typed.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let (heads, hits, seed_rows) = {
            let library = self.state.library.read(cx);
            match library.projection() {
                Some(projection) => {
                    // One mask per refresh, shared by the hits, the heads,
                    // and the browse order below.
                    let seed = self
                        .seed
                        .as_ref()
                        .map(|seed| seed_rows(projection, &seed.ids));
                    if !self.query.is_empty() {
                        let mut heads = Vec::new();
                        for hit in projection
                            .search_artists(&self.query)
                            .into_iter()
                            .take(MAX_ARTIST_HITS)
                        {
                            heads.push(Head::Artist {
                                album_artist: hit.album_artist,
                                row: hit.row,
                            });
                        }
                        for hit in projection
                            .search_albums(&self.query)
                            .into_iter()
                            .take(MAX_ALBUM_HITS)
                        {
                            heads.push(Head::Album {
                                album_artist: hit.album_artist,
                                album: hit.album,
                                row: hit.row,
                            });
                        }
                        let mut hits = projection.search(&self.query);
                        if let Some(rows) = &seed {
                            heads.retain(|head| rows.keeps_head(*head));
                            hits.retain(|&row| rows.keeps(row));
                        }
                        (heads, Arc::new(hits), seed)
                    } else if let Some(rows) = &seed {
                        // A seed with nothing typed is a set to look at, so
                        // it lists in the library's browse order rather than
                        // the shuffle the whole catalog gets.
                        let ordered: Vec<u32> = library
                            .order()
                            .iter()
                            .copied()
                            .filter(|&row| rows.keeps(row))
                            .collect();
                        (Vec::new(), Arc::new(ordered), seed)
                    } else {
                        // Nothing typed yet: offer a random draw instead of the
                        // head of the browse order, which never changed between
                        // opens. The whole order gets shuffled rather than
                        // sampled, so enter on a row still queues a full run
                        // behind it.
                        let mut rows = library.order().as_ref().clone();
                        shuffle_slice(&mut rows);
                        (Vec::new(), Arc::new(rows), seed)
                    }
                }
                None => (Vec::new(), Arc::new(Vec::new()), None),
            }
        };
        self.heads = heads;
        self.hits = hits;
        self.seed_rows = seed_rows;
        self.cover_paths.clear();
        self.selected = HashSet::from([0]);
        self.cursor = 0;
        self.anchor = Some(0);
        self.menu_row = None;
        self.error = None;
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn on_search_event(
        &mut self,
        search: &Entity<SearchBox>,
        event: &SearchEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SearchEvent::Changed => {
                self.query = search.read(cx).query().to_string();
                self.refresh(cx);
            }
            // Nothing lit, nothing to play: a ctrl click can empty the
            // list, and enter answers that with silence rather than with
            // whatever row the cursor was last parked on.
            SearchEvent::Submitted => {
                if !self.selected.is_empty() {
                    self.play(self.cursor, cx);
                }
            }
            // The box's escape ladder ends here: the query is already
            // empty, so escape closes the modal.
            SearchEvent::Dismissed => cx.emit(DismissEvent),
            SearchEvent::FocusChanged => {}
        }
    }

    /// Step the cursor, clamped to the list; the scroll follows only when
    /// the row leaves the view. The selection collapses onto the cursor,
    /// so an arrow after a multi-row click starts over from one row.
    fn move_selected(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.len();
        if len == 0 {
            return;
        }
        let ix = (self.cursor as isize + delta).clamp(0, len as isize - 1) as usize;
        if ix == self.cursor && self.selected.len() == 1 {
            return;
        }
        self.cursor = ix;
        self.selected = HashSet::from([ix]);
        self.anchor = Some(ix);
        self.scroll.scroll_to_item(ix, ScrollStrategy::Top);
        cx.notify();
    }

    /// Ctrl/Cmd+A, hosted: every row in the list, anchored at the top.
    /// The cursor stays where it was, so enter still plays the row the
    /// arrows last landed on.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        let len = self.len();
        if len == 0 {
            return;
        }
        self.selected = (0..len).collect();
        self.anchor = Some(0);
        cx.notify();
    }

    /// Take a row on a click, hosted: the modifiers pick the set through
    /// [`click_selection`] and the cursor lands on the row the click took.
    /// Focus goes back to the search box, since the window's whole shape
    /// is pick a row, then keep typing.
    fn select(
        &mut self,
        ix: usize,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ix >= self.len() {
            return;
        }
        let (selected, anchor) = click_selection(
            &self.selected,
            self.anchor,
            ix,
            modifiers.shift,
            modifiers.secondary(),
        );
        // The cursor lives inside the selection, so a ctrl click that took
        // this row back out can't leave it standing there: enter would
        // play a row that isn't lit. It steps to the nearest row still
        // lit, and when the click emptied the list it stays put with
        // nothing to play, which is what an empty selection means.
        self.cursor = if selected.contains(&ix) {
            ix
        } else {
            nearest_selected(&selected, ix).unwrap_or(self.cursor)
        };
        self.selected = selected;
        self.anchor = anchor;
        let handle = self.search.read(cx).focus_handle(cx);
        window.focus(&handle);
        cx.notify();
    }

    /// The db ids playing a row queues. A track takes itself and the
    /// tracks after it in the result order, same as a double click in the
    /// library; an album or artist head takes its whole run, gathered
    /// from the canonical browse order so discs and tracks come out in
    /// order, and cut to the seed like the list is. Shared by enter, the
    /// click, and the context menu's Play.
    fn play_ids(&self, ix: usize, cx: &App) -> Vec<i64> {
        if ix >= self.len() {
            return Vec::new();
        }
        let library = self.state.library.read(cx);
        let Some(projection) = library.projection() else {
            return Vec::new();
        };
        if ix < self.heads.len() {
            let head = self.heads[ix];
            let seed = self.seed_rows.as_ref();
            library
                .order()
                .iter()
                .copied()
                .filter(|&row| head.contains(projection, row))
                .filter(|&row| seed.is_none_or(|seed| seed.keeps(row)))
                .take(QUEUE_CAP)
                .map(|row| projection.db_id[row as usize])
                .collect()
        } else {
            let track_ix = ix - self.heads.len();
            self.hits[track_ix..]
                .iter()
                .take(QUEUE_CAP)
                .map(|&row| projection.db_id[row as usize])
                .collect()
        }
    }

    /// The highlighted rows as db ids, in list order and without repeats.
    /// A head stands for every track it holds, so the tag editor and the
    /// queue items act on the album a right click landed on. Resolved at
    /// menu-build time, the way the panels do it.
    fn selection_ids(&self, cx: &App) -> Vec<i64> {
        let mut rows: Vec<usize> = self.selected.iter().copied().collect();
        rows.sort_unstable();
        let library = self.state.library.read(cx);
        let Some(projection) = library.projection() else {
            return Vec::new();
        };
        let seed = self.seed_rows.as_ref();
        let mut seen = HashSet::new();
        let mut ids = Vec::new();
        for ix in rows {
            if ix < self.heads.len() {
                let head = self.heads[ix];
                ids.extend(
                    library
                        .order()
                        .iter()
                        .copied()
                        .filter(|&row| head.contains(projection, row))
                        .filter(|&row| seed.is_none_or(|seed| seed.keeps(row)))
                        .map(|row| projection.db_id[row as usize]),
                );
            } else if let Some(&row) = self.hits.get(ix - self.heads.len()) {
                ids.push(projection.db_id[row as usize]);
            }
        }
        ids.retain(|id| seen.insert(*id));
        ids.truncate(QUEUE_CAP);
        ids
    }

    /// Queue a resolved set of ids. As an overlay this also dismisses:
    /// the modal is a jump, so picking a track is the end of it. Hosted,
    /// the window stays up and the results stay put.
    fn queue_ids(&mut self, ids: Vec<i64>, cx: &mut Context<Self>) {
        if ids.is_empty() {
            return;
        }
        let result = self.state.library.read(cx).keys_for(&ids);
        match result {
            Ok(keys) => {
                self.state
                    .player
                    .update(cx, |player, cx| player.play(keys, cx));
                if !self.hosted {
                    cx.emit(DismissEvent);
                }
            }
            Err(e) => {
                self.error = Some(format!("library: {e}").into());
                cx.notify();
            }
        }
    }

    /// Queue the picked entry: what [`play_ids`](Self::play_ids) gathers
    /// for the row, through the one queue path.
    fn play(&mut self, ix: usize, cx: &mut Context<Self>) {
        let ids = self.play_ids(ix, cx);
        self.queue_ids(ids, cx);
    }

    /// The visible slice of the hit list. Row text resolves through the
    /// projection per visible row, so a huge library costs only what
    /// shows.
    fn hit_rows(&mut self, range: std::ops::Range<usize>, cx: &mut Context<Self>) -> Vec<Div> {
        let show_cover = self.config.show_cover;
        let hosted = self.hosted;
        let head_count = self.heads.len();
        let rows: Vec<RowInfo> = {
            let QuickPlay {
                state,
                heads,
                hits,
                cover_paths,
                ..
            } = self;
            let library = state.library.read(cx);
            let Some(projection) = library.projection() else {
                return Vec::new();
            };
            // The cover's path, resolved only when the column shows and cached
            // per id so a scroll doesn't re-query the store every frame.
            let mut cover_path = |row: u32| {
                if !show_cover {
                    return None;
                }
                let id = projection.db_id[row as usize];
                cover_paths
                    .entry(id)
                    .or_insert_with(|| {
                        library
                            .paths_for(&[id])
                            .ok()
                            .and_then(|mut paths| paths.pop())
                    })
                    .clone()
            };
            range
                .filter_map(|ix| {
                    // Heads lead the list, tracks follow; the index splits
                    // on the head count.
                    if ix < head_count {
                        let head = *heads.get(ix)?;
                        let (title, reading, sub, tag) = match head {
                            Head::Artist { album_artist, .. } => (
                                projection.album_artists.strings[album_artist as usize].clone(),
                                projection
                                    .album_artists
                                    .sort_name(album_artist as usize)
                                    .to_string(),
                                String::new(),
                                rox_i18n::t!("quick-play-tag-artist"),
                            ),
                            Head::Album {
                                album_artist,
                                album,
                                ..
                            } => (
                                projection.albums.strings[album as usize].clone(),
                                projection.albums.sort_name(album as usize).to_string(),
                                projection.album_artists.strings[album_artist as usize].clone(),
                                rox_i18n::t!("quick-play-tag-album"),
                            ),
                        };
                        return Some(RowInfo {
                            ix,
                            title: SharedString::from(title),
                            title_reading: SharedString::from(reading),
                            sub: SharedString::from(sub),
                            trailing: tag,
                            path: cover_path(head.row()),
                            is_head: true,
                        });
                    }
                    let row = *hits.get(ix - head_count)?;
                    let v = projection.resolve(row);
                    let sub = match (v.artist.is_empty(), v.album.is_empty()) {
                        (false, false) => format!("{} - {}", v.artist, v.album),
                        (false, true) => v.artist.to_string(),
                        (true, false) => v.album.to_string(),
                        (true, true) => String::new(),
                    };
                    // A zero length is unknown, not a real 0:00 (the
                    // scanner leaves it zero when it can't read a file's
                    // tags), so leave the time blank and drop the cell.
                    let time = if v.duration_ms == 0 {
                        SharedString::default()
                    } else {
                        SharedString::from(fmt_ms(v.duration_ms))
                    };
                    Some(RowInfo {
                        ix,
                        title: SharedString::from(v.title.to_string()),
                        title_reading: SharedString::from(v.title_sort.to_string()),
                        sub: SharedString::from(sub),
                        trailing: time,
                        path: cover_path(row),
                        is_head: false,
                    })
                })
                .collect()
        };
        // Thumbnails, once the library borrow is dropped so the store updates.
        let covers: Vec<Option<Thumb>> = rows
            .iter()
            .map(|info| {
                track_columns::cover_thumb(&self.state, info.path.as_deref(), show_cover, cx)
            })
            .collect();
        let row_h = self.row_h();
        let readings = rox_core::settings::show_readings();
        rows.into_iter()
            .zip(covers)
            .map(|(info, cover)| {
                let RowInfo {
                    ix,
                    title,
                    title_reading,
                    sub,
                    trailing,
                    path: _,
                    is_head,
                } = info;
                div()
                    // Fills the list's width so a long title truncates
                    // inside the modal instead of running the row wide, and
                    // the duration stays pinned to the right edge.
                    .w_full()
                    .h(px(row_h))
                    .px(tokens::SPACE_SM)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .cursor_pointer()
                    .when(self.selected.contains(&ix), |d| {
                        d.bg(palette::alpha(palette::accent(), 0x26))
                    })
                    // Hosted, a set can be lit at once, so the cursor row
                    // takes a touch more accent to say where enter and the
                    // arrows are. The overlay lights the cursor alone and
                    // keeps its single tint.
                    .when(
                        hosted && self.cursor == ix && self.selected.len() > 1,
                        |d| d.bg(palette::alpha(palette::accent(), 0x3d)),
                    )
                    .hover(|d| d.bg(palette::bg_control_hover_opaque()))
                    // The overlay plays on the press, the jump it exists
                    // for; hosted, the press picks rows and the second
                    // click of a double plays, the library's move.
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            if !this.hosted || event.click_count > 1 {
                                this.play(ix, cx);
                            } else {
                                this.select(ix, event.modifiers, window, cx);
                            }
                        }),
                    )
                    // The right press records the row and, outside the
                    // selection, takes it alone, so the menu acts on
                    // what's lit.
                    .when(hosted, |d| {
                        d.on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                this.menu_row = Some(ix);
                                if !this.selected.contains(&ix) {
                                    this.select(ix, Modifiers::default(), window, cx);
                                }
                            }),
                        )
                    })
                    .when(show_cover, |d| {
                        d.child(track_columns::cover_cell(
                            &cover,
                            track_columns::ROW_HEIGHT_STOCK,
                        ))
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .child(div().w_full().truncate().child(panel::named(
                                &title,
                                &title_reading,
                                readings,
                            )))
                            // An empty subtitle drops out entirely rather
                            // than reserving a blank line, so the column is
                            // just the title and the row's items_center
                            // centers it on the row's midline.
                            .when(self.config.show_subtitle && !sub.is_empty(), |d| {
                                d.child(
                                    div()
                                        .w_full()
                                        .truncate()
                                        .text_xs()
                                        .text_color(palette::text_secondary())
                                        .child(sub),
                                )
                            }),
                    )
                    // A head always tags its kind on the right; a track's
                    // duration follows the toggle and drops when unknown.
                    .when(
                        is_head || (self.config.show_duration && !trailing.is_empty()),
                        |d| {
                            d.child(
                                div()
                                    .flex_none()
                                    .text_color(palette::text_muted())
                                    .child(trailing),
                            )
                        },
                    )
            })
            .collect()
    }

    /// One footer chip: a click appends its term to the query and puts
    /// focus back in the box, so narrowing is a click plus typing on.
    fn hint_chip(&self, term: SharedString, cx: &mut Context<Self>) -> Div {
        div()
            .px(tokens::SPACE_XS)
            .rounded(tokens::RADIUS)
            .bg(palette::bg_control())
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control_hover_opaque()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener({
                    let term = term.clone();
                    move |this, _, window, cx| {
                        this.search
                            .update(cx, |search, cx| search.append_term(&term, window, cx));
                    }
                }),
            )
            .child(term)
    }

    /// The footer that makes the query syntax visible: the help button that
    /// opens the syntax sheet, then every `field:` chip, then every bare
    /// `-field` absence, all in one row that wraps to the width it has. No
    /// labels over the groups: the chips read as what they are, and the
    /// sheet is one click away for anyone who wants the words.
    fn hint_row(&self, cx: &mut Context<Self>) -> Div {
        div()
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .border_t_1()
            .border_color(palette::border())
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(tokens::SPACE_XS)
            .text_xs()
            .text_color(palette::text_muted())
            .child(self.syntax_button(cx))
            .children(
                QUERY_FIELDS
                    .iter()
                    .map(|(name, _)| self.hint_chip(SharedString::from(format!("{name}:")), cx)),
            )
            .children(ABSENCE_FIELDS.iter().map(|name| {
                // The trailing space closes the term off: an absence takes
                // no value, so the next thing typed is a new term rather
                // than an accidental `-year:`.
                self.hint_chip(SharedString::from(format!("-{name} ")), cx)
            }))
    }

    /// The footer's help button, tinted while the syntax sheet is open.
    fn syntax_button(&self, cx: &mut Context<Self>) -> Div {
        let on = self.show_syntax;
        div()
            .flex_none()
            .p(px(2.))
            .rounded(tokens::RADIUS)
            .cursor_pointer()
            .when(on, |d| d.bg(palette::bg_control_active()))
            .when(!on, |d| d.hover(|d| d.bg(palette::bg_control())))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.show_syntax = !this.show_syntax;
                    cx.notify();
                }),
            )
            .child(svg().path(icons::INFO).size(px(13.)).text_color(if on {
                palette::text()
            } else {
                palette::text_muted()
            }))
    }

    /// The syntax sheet the help button opens: one line per shape the
    /// query takes, the example on the left in a control chip and what it
    /// does beside it. Clicking a line appends its example, the same path
    /// the footer chips take, so the sheet teaches by handing over a
    /// working fragment.
    fn syntax_sheet(&self, cx: &mut Context<Self>) -> Div {
        div()
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .border_t_1()
            .border_color(palette::border())
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .text_xs()
            .text_color(palette::text_muted())
            .child(rox_i18n::t!("quick-play-syntax-title"))
            .children(SYNTAX_KEYS.iter().map(|key| {
                let example = rox_i18n::t!(&format!("{key}.example"));
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener({
                            let example = example.clone();
                            move |this, _, window, cx| {
                                this.search.update(cx, |search, cx| {
                                    search.append_term(&example, window, cx)
                                });
                            }
                        }),
                    )
                    .child(
                        div()
                            .flex_none()
                            .px(tokens::SPACE_XS)
                            .rounded(tokens::RADIUS)
                            .bg(palette::bg_control())
                            .text_color(palette::text())
                            .child(example),
                    )
                    .child(div().flex_1().min_w_0().child(rox_i18n::t!(*key)))
            }))
    }

    /// The seed's chip under the search row: the name of the set the
    /// search is running inside, and the x that drops back to the whole
    /// library.
    fn seed_chip(&self, label: SharedString, cx: &mut Context<Self>) -> Div {
        div()
            .pt(tokens::SPACE_SM)
            .flex()
            .flex_row()
            .items_center()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(tokens::SPACE_XS)
                    .pl(tokens::SPACE_XS)
                    .pr(px(3.))
                    .py(px(1.))
                    .rounded(tokens::RADIUS)
                    .bg(palette::bg_control())
                    .text_xs()
                    .text_color(palette::text())
                    .child(label)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.set_seed(None, cx)),
                            )
                            .child(
                                svg()
                                    .path(icons::CLOSE)
                                    .size(px(10.))
                                    .text_color(palette::text_muted()),
                            ),
                    ),
            )
    }

    /// The settings button beside the search box: a sliders glyph that
    /// opens the config panel, tinted while it's open.
    fn config_button(&self, cx: &mut Context<Self>) -> Div {
        let on = self.show_config;
        div()
            .flex_none()
            .p(tokens::SPACE_XS)
            .rounded(tokens::RADIUS)
            .cursor_pointer()
            .when(on, |d| d.bg(palette::bg_control_active()))
            .when(!on, |d| d.hover(|d| d.bg(palette::bg_control())))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.toggle_config(cx)),
            )
            .child(svg().path(icons::SLIDERS).size(px(16.)).text_color(if on {
                palette::text()
            } else {
                palette::text_muted()
            }))
    }

    /// The inline config panel that drops under the search row when the
    /// settings button is on: the modal's appearance knobs, each writing
    /// straight through to settings.
    fn config_panel(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .pt(tokens::SPACE_SM)
            .mt(tokens::SPACE_SM)
            .border_t_1()
            .border_color(palette::border())
            .child(panel::setting_row(
                rox_i18n::t!("quick-play-cover"),
                Some(rox_i18n::t!("quick-play-cover.description")),
                panel::toggle(
                    self.config.show_cover,
                    |this: &mut Self, on, cx| {
                        this.edit_config(|c| c.show_cover = on, cx);
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("quick-play-subtitle"),
                Some(rox_i18n::t!("quick-play-subtitle.description")),
                panel::toggle(
                    self.config.show_subtitle,
                    |this: &mut Self, on, cx| {
                        this.edit_config(|c| c.show_subtitle = on, cx);
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("quick-play-duration"),
                Some(rox_i18n::t!("quick-play-duration.description")),
                panel::toggle(
                    self.config.show_duration,
                    |this: &mut Self, on, cx| {
                        this.edit_config(|c| c.show_duration = on, cx);
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("quick-play-comfortable-rows"),
                Some(rox_i18n::t!("quick-play-comfortable-rows.description")),
                panel::toggle(
                    self.config.comfortable,
                    |this: &mut Self, on, cx| {
                        this.edit_config(|c| c.comfortable = on, cx);
                    },
                    cx,
                ),
            ))
    }
}

impl Render for QuickPlay {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let len = self.len();
        // As an overlay the list is exactly as tall as the rows it shows,
        // up to the visible cap, and the modal sizes itself around it.
        // Hosted in a window there's a frame to fill instead, so the list
        // takes whatever the search row and footer leave.
        let list_h: gpui::Length = if self.hosted {
            relative(1.).into()
        } else {
            px(self.row_h() * len.clamp(1, VISIBLE_ROWS) as f32).into()
        };
        let this = cx.entity().downgrade();
        let list = if len == 0 {
            div()
                .h(list_h)
                .flex()
                .items_center()
                .justify_center()
                .text_color(palette::text_muted())
                .child(if self.query.is_empty() {
                    "The library is empty"
                } else {
                    "No matches"
                })
                .into_any_element()
        } else {
            uniform_list("quick-play-hits", len, move |range, _, cx| {
                this.upgrade()
                    .map(|this| this.update(cx, |this, cx| this.hit_rows(range, cx)))
                    .unwrap_or_default()
            })
            .track_scroll(self.scroll.clone())
            .h(list_h)
            .w_full()
            .into_any_element()
        };
        // Hosted, the list sits in a flex child that soaks up the leftover
        // height and carries the row context menu; as an overlay it's
        // already the height it wants, and a menu there would fight the
        // click-outside dismiss.
        let list = if self.hosted {
            let weak = cx.entity().downgrade();
            div()
                .flex_1()
                .min_h_0()
                .child(list)
                // A right press arrives here in the capture phase, before
                // any row's bubble handler records itself, so a press off
                // the rows leaves no target and the menu below comes up
                // empty rather than acting on a stale row.
                .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
                    if event.button == MouseButton::Right {
                        this.menu_row = None;
                    }
                }))
                .context_menu(move |menu, window, cx| {
                    let Some(this) = weak.upgrade() else {
                        return menu;
                    };
                    let Some(ix) = ({
                        let view = this.read(cx);
                        view.menu_row.filter(|ix| *ix < view.len())
                    }) else {
                        return menu;
                    };
                    let view = this.read(cx);
                    // The right press already pulled the row into the set,
                    // so this is what's lit.
                    let ids = view.selection_ids(cx);
                    let rows = view.selected.len();
                    let label = if rows > 1 {
                        rox_i18n::t!("library-play-tracks", count = rows as u64).to_string()
                    } else {
                        match view.heads.get(ix) {
                            Some(Head::Album { .. }) => {
                                rox_i18n::t!("library-play-album").to_string()
                            }
                            Some(Head::Artist { .. }) => {
                                rox_i18n::t!("library-play-group").to_string()
                            }
                            None => rox_i18n::t!("library-play").to_string(),
                        }
                    };
                    let state = view.state.clone();
                    let play = this.downgrade();
                    // One row plays from it through the results, the
                    // double click's move; a set queues exactly what's
                    // lit.
                    panel::track_actions(menu, state, ids, label, window, cx, move |_, cx| {
                        let Some(this) = play.upgrade() else {
                            return;
                        };
                        this.update(cx, |this, cx| {
                            if this.selected.len() > 1 {
                                let ids = this.selection_ids(cx);
                                this.queue_ids(ids, cx);
                            } else {
                                this.play(ix, cx);
                            }
                        });
                    })
                })
                .into_any_element()
        } else {
            list
        };
        div()
            .when(self.hosted, |d| d.size_full())
            .when(!self.hosted, |d| d.w(px(560.)))
            .flex()
            .flex_col()
            .bg(palette::bg_menu_opaque())
            // The card edge is what lifts the modal off the workspace
            // behind it; a window has its own frame, and a click outside
            // the view lands on that frame rather than on something the
            // modal is covering, so neither is wanted hosted.
            .when(!self.hosted, |d| {
                d.rounded(tokens::RADIUS)
                    .border_1()
                    .border_color(palette::border_light())
                    .shadow_md()
                    .occlude()
                    .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(DismissEvent)))
            })
            // Scopes the workspace's playback key bindings out while the
            // modal is up, so space and arrows work the query and the
            // list instead.
            .key_context("SearchInput")
            // The input binds up/down (and page keys) to its own cursor
            // actions and swallows them without propagating on a single
            // line, so the list takes them in the capture phase before
            // they reach it, unless the suggestion menu is open, which
            // gets them first so it stays navigable.
            .capture_action(cx.listener(|this, _: &MoveUp, window, cx| {
                if !this.menu_action(Box::new(MoveUp), window, cx) {
                    this.move_selected(-1, cx);
                }
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, window, cx| {
                if !this.menu_action(Box::new(MoveDown), window, cx) {
                    this.move_selected(1, cx);
                }
            }))
            .capture_action(
                cx.listener(|this, _: &MovePageUp, _, cx| this.move_selected(-PAGE_ROWS, cx)),
            )
            .capture_action(
                cx.listener(|this, _: &MovePageDown, _, cx| this.move_selected(PAGE_ROWS, cx)),
            )
            // The input binds the select-all chord to its own text, and
            // the search box keeps focus by design, so the list has to
            // take the action ahead of it. Hosted, the window is for
            // picking rows, so the chord takes them all. The overlay has
            // no set to take, so the input keeps its text select.
            .capture_action(cx.listener(|this, _: &SelectAll, _, cx| {
                if this.hosted {
                    this.select_all(cx);
                } else {
                    cx.propagate();
                }
            }))
            // The search box handles escape while it has focus (its clear
            // then dismiss ladder); this catches an escape from anywhere
            // else in the modal.
            .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.emit(DismissEvent);
                }
            }))
            .child(
                div()
                    .p(tokens::SPACE_SM)
                    .border_b_1()
                    .border_color(palette::border())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(self.search.update(cx, |search, cx| search.element(cx))),
                            )
                            .child(self.config_button(cx)),
                    )
                    .when_some(
                        self.seed.as_ref().map(|seed| seed.label.clone()),
                        |d, label| d.child(self.seed_chip(label, cx)),
                    )
                    .when(self.show_config, |d| d.child(self.config_panel(cx))),
            )
            .child(list)
            .when(self.show_syntax, |d| d.child(self.syntax_sheet(cx)))
            .child(self.hint_row(cx))
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

#[cfg(test)]
mod tests {
    use super::*;
    use rox_library::rusqlite::Connection;
    use rox_library::{store, TrackRow};

    /// One scanned row with just the fields the seed reads set.
    fn track(path: &str, album_artist: &str, album: &str) -> TrackRow {
        TrackRow {
            path: path.into(),
            sub: 0,
            cue: None,
            title: "Song".into(),
            artist: album_artist.into(),
            album_artist: album_artist.into(),
            album: album.into(),
            title_sort: String::new(),
            artist_sort: String::new(),
            album_artist_sort: String::new(),
            album_sort: String::new(),
            genre: String::new(),
            year: 0,
            disc_no: 1,
            track_no: 1,
            duration_ms: 200_000,
            codec: "mp3".into(),
            bitrate_kbps: 320,
            sample_rate_hz: 44100,
            bit_depth: 0,
            rating: 0,
            replay_gain: Default::default(),
            bpm: None,
            size: 0,
            mtime: 0,
        }
    }

    /// A projection over an in-memory database seeded with the rows, the
    /// same path the app builds its read model over.
    fn projection(rows: &[TrackRow]) -> Projection {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, rows).unwrap();
        Projection::load_serial(&conn, false).unwrap()
    }

    /// A seed of two tracks out of four: the mask takes exactly those
    /// rows, and the groups cover the artists and albums they sit in, so
    /// a head whose tracks are all outside the seed is dropped even
    /// though the search found it.
    #[test]
    fn a_seed_keeps_its_own_rows_and_the_groups_they_sit_in() {
        let p = projection(&[
            track("/m/a/1.mp3", "In", "First"),
            track("/m/a/2.mp3", "In", "Second"),
            track("/m/b/1.mp3", "Out", "Third"),
            track("/m/b/2.mp3", "Out", "Fourth"),
        ]);
        let row_of = |album: &str| {
            (0..p.len() as u32)
                .find(|&row| p.albums.strings[p.album[row as usize] as usize] == album)
                .expect("row present")
        };
        let (kept, dropped) = (row_of("First"), row_of("Third"));
        let seed = seed_rows(&p, &[p.db_id[kept as usize]]);

        assert!(seed.keeps(kept));
        assert!(!seed.keeps(dropped));
        // Past the mask's end is outside, not a panic.
        assert!(!seed.keeps(p.len() as u32 + 10));

        let head = |row: u32| Head::Album {
            album_artist: p.album_artist[row as usize],
            album: p.album[row as usize],
            row,
        };
        assert!(seed.keeps_head(head(kept)));
        assert!(!seed.keeps_head(head(dropped)));
        // The artist head follows the same rule: "In" has a seeded track,
        // "Out" has none, even though only one of In's two albums is in.
        assert!(seed.keeps_head(Head::Artist {
            album_artist: p.album_artist[kept as usize],
            row: kept,
        }));
        assert!(!seed.keeps_head(Head::Artist {
            album_artist: p.album_artist[dropped as usize],
            row: dropped,
        }));
    }

    /// The clicks the hosted list takes: plain lands on one row, ctrl or
    /// cmd toggles a row in and out without touching the rest, shift takes
    /// the run from the anchor, in either direction, and ctrl+shift stacks
    /// that run onto what's lit.
    #[test]
    fn a_click_takes_a_row_a_toggle_or_a_range() {
        let (set, anchor) = click_selection(&HashSet::new(), None, 3, false, false);
        assert_eq!(set, HashSet::from([3]));
        assert_eq!(anchor, Some(3));

        let (set, anchor) = click_selection(&set, anchor, 5, false, true);
        assert_eq!(set, HashSet::from([3, 5]));
        assert_eq!(anchor, Some(5));
        // The same row again takes itself back out.
        let (off, _) = click_selection(&set, anchor, 5, false, true);
        assert_eq!(off, HashSet::from([3]));

        // Shift measures from the anchor and keeps it, so a second shift
        // click re-runs the range rather than growing from where the last
        // one ended.
        let (set, anchor) = click_selection(&set, Some(2), 4, true, false);
        assert_eq!(set, HashSet::from([2, 3, 4]));
        assert_eq!(anchor, Some(2));
        let (set, anchor) = click_selection(&set, anchor, 0, true, false);
        assert_eq!(set, HashSet::from([0, 1, 2]));
        assert_eq!(anchor, Some(2));

        // Shift with nothing anchored yet is just that row.
        let (set, anchor) = click_selection(&HashSet::new(), None, 7, true, false);
        assert_eq!(set, HashSet::from([7]));
        assert_eq!(anchor, Some(7));

        // Ctrl+shift keeps the first block and adds the run from the
        // anchor, so two separate ranges can be picked.
        let (set, anchor) = click_selection(&HashSet::from([0, 1]), Some(5), 7, true, true);
        assert_eq!(set, HashSet::from([0, 1, 5, 6, 7]));
        assert_eq!(anchor, Some(5));
    }

    /// A toggle that takes the clicked row out has to hand the cursor
    /// somewhere still lit, since enter plays the cursor and a cursor
    /// outside the selection plays a row nobody picked.
    #[test]
    fn the_cursor_follows_a_toggle_off_to_a_lit_row() {
        let set = HashSet::from([1, 4, 9]);
        assert_eq!(nearest_selected(&set, 5), Some(4));
        assert_eq!(nearest_selected(&set, 7), Some(9));
        // A tie reads upward, the row the eye is already on.
        assert_eq!(nearest_selected(&HashSet::from([2, 4]), 3), Some(2));
        // Nothing lit, nothing to move to; enter answers with silence.
        assert_eq!(nearest_selected(&HashSet::new(), 3), None);
    }

    /// An empty seed is a real narrowing to nothing, not "no seed": the
    /// mask has to come out all false rather than None-and-pass-all.
    #[test]
    fn an_empty_seed_keeps_nothing() {
        let p = projection(&[track("/m/a/1.mp3", "In", "First")]);
        let seed = seed_rows(&p, &[]);
        assert!(!seed.keeps(0));
        assert!(seed.artists.is_empty());
        assert!(seed.albums.is_empty());
    }
}
