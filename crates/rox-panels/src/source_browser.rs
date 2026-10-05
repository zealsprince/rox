//! The source browser: one plugin source's tree, browsed, searched and
//! synced (ADR 30). It's a core panel pinned to a `plugin:<id>` source id,
//! and the plugin supplies none of it; every live call goes through
//! [`rox_services::plugins`] on the background executor.
//!
//! A synced collection opens from the library through its membership with no
//! plugin call, so it still browses with the plugin stopped or the network
//! down. A browsed track only becomes a library row when it's played, queued
//! or added to a playlist, through a pick.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, AnyElement, AnyWindowHandle, App, Axis, Context, Div, ElementId,
    EventEmitter, FocusHandle, Focusable, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent,
    ObjectFit, Pixels, ScrollStrategy, SharedString, Subscription, Task, WeakEntity, Window,
    canvas, div, ease_out_quint, img, prelude::*, px, size, svg,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::spinner::Spinner;
use gpui_component::{Icon, Sizable, VirtualListScrollHandle, v_virtual_list};
use rox_core::QUEUE_CAP;
use rox_core::fmt::fmt_num;
use rox_core::settings::{Settings, SyncedCollection};
use rox_dock::{Panel, PanelEvent, PanelView, TabPanel};
use rox_library::cue::{PLUGIN_PREFIX, TrackKey, source_id};
use rox_library::members::{self, PluginTrack};
use rox_panel_api::plugin_actions;
use rox_panel_api::toast::Toast;
use rox_services::plugins::{
    self, Entry, Field, FieldKind, FieldValue, GoTo, NodeKind, Notice, NoticeLink, Page, RadioSeed,
    Target, Unavailable, Values, View,
};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, FlickState, PanelChrome, PanelSettings, Then, Tone, WithIds};
use crate::panel_settings;
use crate::player::fmt_time;
use crate::playing_bars::{self, PlayingBars};
use crate::query::search::{SearchBox, SearchEvent};
use crate::selection::SelectionEvent;
use crate::settings::ui::{IconButton, SmallButton, icon_button};
use crate::thumbs::Thumb;

/// Every row is two lines tall, node or track.
const ROW_H: Pixels = px(40.);

/// A shelf's cover, and the shelf: the cover, two lines under it, and air.
const TILE: Pixels = px(112.);
const SHELF_H: Pixels = px(172.);

/// Tiles to assume across the list before the first paint has measured it.
const FALLBACK_COLS: usize = 4;

const ART: Pixels = px(32.);

/// Rows share it so the keep button shows on the hovered row only.
const NODE_ROW: &str = "source-node-row";

/// The same for a track row's library check.
const TRACK_ROW: &str = "source-track-row";

/// How far a place loading over the old one dims it.
const VEIL: f32 = 0.55;

/// Where a track stands with the library.
#[derive(Clone, Copy, PartialEq)]
enum Held {
    Out,
    /// Added on its own, so its check takes it back out.
    Saved,
    /// Held by a kept collection, whose own switch takes it out.
    Kept,
}

/// The same for tiles.
const NODE_TILE: &str = "source-node-tile";

/// Where a keep button sits: beside a row's chevron, or over a tile's cover.
#[derive(Clone, Copy, PartialEq)]
enum KeepOn {
    Row,
    Tile,
}

/// A field's column: a short number or a date.
const FIELD_W: Pixels = px(64.);

/// Three digits, for the rare album that runs past 99.
const TRACK_NO_W: Pixels = px(20.);

/// Sorting by a field reads the rest of the list first, up to this many rows.
const SORT_CAP: usize = 1000;

/// Tracks the home shows after the playing one.
const UP_NEXT: usize = 3;

/// Up Next folded to its first track on one line.
const UP_NEXT_LINE_H: Pixels = px(28.);
const UP_NEXT_ART: Pixels = px(20.);

/// The tracks a play queues: up to `cap` around the clicked one, half of
/// them behind it for Prev, the rest ahead, a short side's share going to
/// the other. Answers the range and where the clicked track sits in it.
fn play_window(len: usize, at: usize, cap: usize) -> (std::ops::Range<usize>, usize) {
    let lo = at.saturating_sub(cap / 2);
    let hi = (lo + cap).min(len);
    let lo = hi.saturating_sub(cap).min(lo);

    (lo..hi, at - lo)
}

/// What the list draws, one item each: a row for an entry, a shelf for the
/// run of entries under a tiles section, or one line of that run wrapped.
#[derive(Clone, Debug, PartialEq)]
enum Visual {
    Row(usize),
    Shelf(std::ops::Range<usize>),
    Line(std::ops::Range<usize>),
}

/// A page that's one tiles section and nothing else wraps into lines of
/// `cols`. With no rows to stack against, a shelf would hide most of it.
fn layout(entries: &[Entry], cols: usize) -> Vec<Visual> {
    let mut out = Vec::new();
    let mut ix = 0;

    while ix < entries.len() {
        out.push(Visual::Row(ix));

        if matches!(entries[ix], Entry::Section { tiles: true, .. }) {
            let start = ix + 1;
            let end = entries[start..]
                .iter()
                .position(|entry| matches!(entry, Entry::Section { .. }))
                .map_or(entries.len(), |at| start + at);

            if end > start {
                out.push(Visual::Shelf(start..end));
            }
            ix = end;
        } else {
            ix += 1;
        }
    }

    if let [Visual::Row(0), Visual::Shelf(run)] = out.as_slice() {
        let run = run.clone();
        let cols = cols.max(1);

        return std::iter::once(Visual::Row(0))
            .chain(
                run.clone()
                    .step_by(cols)
                    .map(|start| Visual::Line(start..(start + cols).min(run.end))),
            )
            .collect();
    }

    out
}

/// The item an entry is drawn in.
fn visual_of(visuals: &[Visual], ix: usize) -> Option<usize> {
    visuals.iter().position(|visual| match visual {
        Visual::Row(row) => *row == ix,
        Visual::Shelf(range) | Visual::Line(range) => range.contains(&ix),
    })
}

/// The shelf a tiles heading sits over, if the next item is one.
fn shelf_under(visuals: &[Visual], heading: usize) -> Option<std::ops::Range<usize>> {
    visuals.iter().find_map(|visual| match visual {
        Visual::Shelf(run) if run.start == heading + 1 => Some(run.clone()),
        _ => None,
    })
}

/// Moves a shelf sideways by `by`, held inside its ends.
fn slide(shelf: &gpui::ScrollHandle, by: Pixels) {
    let mut offset = shelf.offset();
    let reach = shelf.max_offset().width;
    offset.x = (offset.x + by).clamp(-reach, px(0.));
    shelf.set_offset(offset);
}

/// A kept collection's second line: the plugin's own, then how many of its
/// tracks the library holds.
fn kept_line(subtitle: &str, count: usize) -> SharedString {
    let count = rox_i18n::t!("source-browser-members", count = count as u64);
    match subtitle.is_empty() {
        true => count,
        false => format!("{subtitle}, {count}").into(),
    }
}

fn node_glyph(kind: Option<NodeKind>, collection: bool) -> &'static str {
    match (kind, collection) {
        (Some(NodeKind::Album), _) => icons::DISC,
        (Some(NodeKind::Playlist), _) | (None, true) => icons::LIST_MUSIC,
        (Some(NodeKind::Artist), _) => icons::USER,
        (Some(NodeKind::Folder), _) | (None, false) => icons::FOLDER,
    }
}

/// The browser whose track menu opened last. Go to picked from the shared
/// menu inside it opens there, not in another browser on the same plugin.
struct MenuOrigin(WeakEntity<SourceBrowserPanel>);

impl gpui::Global for MenuOrigin {}

/// Go to from a menu outside a browser's own: the browser the menu opened in
/// when it shows that plugin, else the first one on it in any tab group,
/// else a new one in the newest group.
pub fn go_to_source(
    state: AppState,
    source: String,
    target: Target,
    window: &mut Window,
    cx: &mut App,
) {
    let origin = cx
        .try_global::<MenuOrigin>()
        .and_then(|origin| origin.0.upgrade())
        .filter(|browser| browser.read(cx).config.source == source);
    if let Some(browser) = origin {
        browser.update(cx, |this, cx| this.open_target(&target, cx));

        // The menu may have opened elsewhere since, so its tab comes forward.
        let tabs = browser
            .read(cx)
            .tab_panel
            .as_ref()
            .and_then(|tabs| tabs.upgrade());
        if let Some(tabs) = tabs {
            let panel: Arc<dyn PanelView> = Arc::new(browser);
            tabs.update(cx, |tabs, cx| tabs.focus_panel(&panel, window, cx));
        }
        return;
    }

    for tabs in state.tab_hosts.read(cx).groups() {
        let found = tabs.read(cx).panels().iter().find_map(|panel| {
            let browser = panel.view().downcast::<SourceBrowserPanel>().ok()?;
            (browser.read(cx).config.source == source).then(|| (panel.clone(), browser))
        });

        if let Some((panel, browser)) = found {
            browser.update(cx, |this, cx| this.open_target(&target, cx));
            tabs.update(cx, |tabs, cx| tabs.focus_panel(&panel, window, cx));
            return;
        }
    }

    let Some(tabs) = state.tab_hosts.read(cx).last_live(cx) else {
        log::warn!("Go to {}: no tab group to open {source} in", target.id);
        return;
    };

    let config = SourceBrowserConfig {
        source,
        ..SourceBrowserConfig::default()
    };
    let browser = cx.new(|cx| SourceBrowserPanel::new(state.clone(), config, window, cx));
    browser.update(cx, |this, cx| this.open_target(&target, cx));
    tabs.update(cx, |tabs, cx| tabs.add_panel(Arc::new(browser), window, cx));
}

fn go_to_item(
    label: SharedString,
    target: Target,
    panel: WeakEntity<SourceBrowserPanel>,
) -> PopupMenuItem {
    PopupMenuItem::new(label)
        .icon(Icon::default().path(node_glyph(target.kind, target.collection)))
        .on_click(move |_, _, cx| {
            panel
                .update(cx, |this, cx| this.go(this.gone_to(&target), cx))
                .ok();
        })
}

/// How a node's tracks go into the queue.
#[derive(Clone, Copy)]
enum NodePlay {
    Play,
    Next,
    Queue,
}

/// How near the end of the list a scroll gets before the next page is asked
/// for.
const PAGE_AHEAD: usize = 20;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceBrowserConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// `plugin:<id>`. Empty shows a picker over the running sources.
    pub source: String,
    /// The source's home listed under the roots rather than as a node there.
    pub home: bool,
}

impl Default for SourceBrowserConfig {
    fn default() -> Self {
        SourceBrowserConfig {
            chrome: PanelChrome::default(),
            source: String::new(),
            home: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Crumb {
    id: String,
    title: String,
    /// Set on the crumb search results leave when one of them opens, so the
    /// way back to them stays.
    query: Option<String>,
    /// The node can be kept, so the place offers the switch.
    collection: bool,
    /// An album's tracks show their numbers.
    kind: Option<NodeKind>,
}

/// Where the list is: a path down the tree, or a search's results, in one
/// of the views the plugin offers there.
#[derive(Clone, Debug, Default, PartialEq)]
struct Place {
    trail: Vec<Crumb>,
    query: Option<String>,
    /// None for the plugin's default view.
    view: Option<String>,
    /// The plugin's part of the library, read from rox alone. The trail
    /// below it is kept collections.
    library: bool,
}

impl Place {
    fn library() -> Place {
        Place {
            library: true,
            ..Place::default()
        }
    }

    /// The node being listed. None at the roots and for a search.
    fn node(&self) -> Option<&Crumb> {
        match self.query {
            Some(_) => None,
            None => self.trail.last().filter(|crumb| crumb.query.is_none()),
        }
    }

    /// The plugin's own roots. The library's top isn't one: it has no home
    /// to follow and nothing the plugin answers.
    fn is_root(&self) -> bool {
        self.trail.is_empty() && self.query.is_none() && !self.library
    }

    /// Where Go to's node goes. A node already on the trail is stepped back
    /// to. An album or artist page gives way to the next one, so hopping
    /// between them doesn't stack crumbs. Anywhere else it opens on from the
    /// place shown, so the way back is where the track was.
    fn going_to(mut self, target: &Target) -> Place {
        if let Some(at) = self
            .trail
            .iter()
            .position(|crumb| crumb.query.is_none() && crumb.id == target.id)
        {
            self.trail.truncate(at + 1);
            self.query = None;
            self.view = None;
            return self;
        }

        if self
            .node()
            .is_some_and(|crumb| matches!(crumb.kind, Some(NodeKind::Album | NodeKind::Artist)))
        {
            self.trail.pop();
        }

        self.opening(Crumb {
            id: target.id.clone(),
            title: target.title.clone(),
            query: None,
            collection: target.collection,
            kind: target.kind,
        })
    }

    /// This place with `crumb` opened from it.
    fn opening(mut self, crumb: Crumb) -> Place {
        self.view = None;

        // Results a node opened from stay a crumb to go back to.
        if let Some(query) = self.query.take() {
            self.trail.push(Crumb {
                id: String::new(),
                title: query.clone(),
                query: Some(query),
                collection: false,
                kind: None,
            });
        }

        self.trail.push(crumb);
        self
    }
}

/// What an answer did to the list.
#[derive(Debug, PartialEq)]
enum Landed {
    /// A newer request replaced the one this answered.
    Stale,
    /// A first page. `moved` is false when it re-read the place already shown.
    Replaced {
        moved: bool,
    },
    Appended,
    Failed,
}

/// The place a play started from, and what it sent, so Home can lead back
/// while one of those tracks plays.
struct PlayedFrom {
    place: Place,
    keys: HashSet<String>,
}

struct Pending {
    generation: u64,
    /// Some for a first page, which replaces the list and moves it there;
    /// None for the next page of the list shown.
    place: Option<Place>,
    /// This is the first page of the home that follows the roots.
    home: bool,
}

/// The source's home under the roots: its node taken out of them, listed
/// once the roots run out, then paged like the rest.
#[derive(Clone, Debug, Default, PartialEq)]
enum Home {
    #[default]
    None,
    Next(String),
    Paging(String),
}

/// The list and its paging. A failed request leaves the last good list up,
/// so the reason shows above it rather than in place of it.
#[derive(Default)]
struct Listing {
    place: Place,
    entries: Vec<Entry>,
    cursor: Option<String>,
    error: Option<String>,
    /// Why the source couldn't answer, when rox knows. Said in place of the
    /// raw error, and the list asks again once the source is back.
    unavailable: Option<Unavailable>,
    /// The plugin's own line over the place shown, from its first page.
    notice: Option<Notice>,
    /// The views the place's first page offered, and which one it is.
    views: Vec<View>,
    view: Option<String>,
    /// The first page's columns, and each entry's values, lined up with
    /// `entries` and sorted with them.
    fields: Vec<Field>,
    values: Vec<Values>,
    /// Every page's Go to nodes, by track key.
    go_to: HashMap<String, GoTo>,
    /// Every page's row flags, by track key or node id.
    flags: HashMap<String, Vec<String>>,
    /// The field the list is sorted by, None for the plugin's order.
    sort: Option<String>,
    home: Home,
    pending: Option<Pending>,
    generation: u64,
}

impl Listing {
    fn begin(&mut self, place: Option<Place>) -> u64 {
        self.begin_page(place, false)
    }

    fn begin_page(&mut self, place: Option<Place>, home: bool) -> u64 {
        self.generation += 1;
        self.pending = Some(Pending {
            generation: self.generation,
            place,
            home,
        });

        self.generation
    }

    /// Takes the home node out of the roots, so its page follows them.
    fn follow_home(&mut self) {
        let Some(at) = self
            .entries
            .iter()
            .position(|entry| matches!(entry, Entry::Node { home: true, .. }))
        else {
            return;
        };

        if let Entry::Node { id, .. } = self.entries.remove(at) {
            self.values.remove(at);
            self.home = Home::Next(id);
        }
    }

    /// What the next page is: a node (None for the place's own) and a cursor,
    /// and whether it's the home's first page.
    fn next_page(&self) -> (Option<String>, Option<String>, bool) {
        match (&self.cursor, &self.home) {
            (None, Home::Next(id)) => (Some(id.clone()), None, true),
            (cursor, Home::Paging(id)) => (Some(id.clone()), cursor.clone(), false),
            (cursor, _) => (None, cursor.clone(), false),
        }
    }

    fn land(&mut self, generation: u64, result: Result<Page, String>) -> Landed {
        let Some(pending) = self.pending.take_if(|p| p.generation == generation) else {
            return Landed::Stale;
        };

        let page = match result {
            Ok(page) => page,

            // The cursor stays, so the same page can be asked for again.
            Err(e) => {
                self.error = Some(e);
                return Landed::Failed;
            }
        };

        self.cursor = page.cursor;
        self.error = None;
        self.unavailable = None;

        match pending.place {
            Some(place) => {
                let moved = place != self.place;
                self.place = place;
                self.entries = page.entries;
                self.notice = page.notice;
                self.views = page.views;
                self.view = page.view;
                self.fields = page.fields;
                self.values = page.values;
                self.values.resize(self.entries.len(), Values::new());
                self.go_to = page.go_to;
                self.flags = page.flags;
                self.sort = None;
                self.home = Home::None;
                Landed::Replaced { moved }
            }

            None => {
                // The home's columns stay out of the roots': they'd land after
                // the roots and narrow every row already drawn.
                if pending.home
                    && let Home::Next(id) = &self.home
                {
                    self.home = Home::Paging(id.clone());
                }

                self.entries.extend(page.entries);
                self.values.extend(page.values);
                self.values.resize(self.entries.len(), Values::new());
                self.go_to.extend(page.go_to);
                self.flags.extend(page.flags);
                Landed::Appended
            }
        }
    }

    /// Stable, and within each run between sections: a heading keeps the
    /// rows under it. Rows without a value go last.
    fn apply_sort(&mut self) {
        let Some(field) = self
            .sort
            .as_ref()
            .and_then(|id| self.fields.iter().find(|f| &f.id == id))
        else {
            return;
        };
        let (id, kind) = (field.id.clone(), field.kind);

        let mut rows: Vec<(Entry, Values)> = std::mem::take(&mut self.entries)
            .into_iter()
            .zip(std::mem::take(&mut self.values))
            .collect();

        let mut start = 0;
        while start < rows.len() {
            let heading = matches!(rows[start].0, Entry::Section { .. });
            let from = start + usize::from(heading);
            let end = rows[from..]
                .iter()
                .position(|(entry, _)| matches!(entry, Entry::Section { .. }))
                .map_or(rows.len(), |at| from + at);

            rows[from..end]
                .sort_by(|(_, a), (_, b)| field_order(kind, value_of(a, &id), value_of(b, &id)));
            start = end.max(start + 1);
        }

        (self.entries, self.values) = rows.into_iter().unzip();
    }

    /// A failed page doesn't retry on its own; reopening the place does.
    fn wants_more(&self) -> bool {
        self.pending.is_none() && self.error.is_none() && self.unread()
    }

    /// Pages left: the place's own, or a home still to follow it.
    fn unread(&self) -> bool {
        self.cursor.is_some() || matches!(self.home, Home::Next(_))
    }

    fn loading(&self) -> bool {
        self.pending.is_some()
    }

    /// A new place is on its way, as opposed to the next page of this one.
    fn navigating(&self) -> Option<u64> {
        self.pending
            .as_ref()
            .filter(|pending| pending.place.is_some())
            .map(|pending| pending.generation)
    }

    /// The place asked for last, whether or not it has landed.
    fn target(&self) -> &Place {
        self.pending
            .as_ref()
            .and_then(|pending| pending.place.as_ref())
            .unwrap_or(&self.place)
    }
}

pub struct SourceBrowserPanel {
    state: AppState,
    config: SourceBrowserConfig,
    label: SharedString,
    /// The record's switched-on collections, read at the list's cadence.
    synced: Vec<SyncedCollection>,
    /// Rows each synced collection holds, by node id.
    members: HashMap<String, usize>,
    listing: Listing,
    /// Library ids of listed tracks that are already rows, by key. Resolved
    /// when the list changes, so a click never queries per row.
    ids: HashMap<String, i64>,
    /// Collections whose switch is still working.
    syncing: HashSet<String>,
    /// How many flag reports from the plugin's actions the listing has merged.
    flags_seen: u64,
    /// Track keys a pick is still adding.
    picking: HashSet<String>,
    playing: Option<TrackKey>,
    opening: Option<TrackKey>,
    /// The player isn't paused. The bars park on a pause, so a resume has to
    /// wake them.
    audible: bool,
    bars: Rc<RefCell<PlayingBars>>,
    played_from: Option<PlayedFrom>,
    /// The next page to land scrolls to the playing row.
    reveal_playing: bool,
    /// The first row of what landed last and when, so new rows fade in.
    fresh: Option<(usize, Instant)>,
    /// What plays after the audible track, read when the queue or the track
    /// moves.
    up_next: Vec<(u64, TrackKey)>,
    up_next_open: bool,
    queue_rev: Option<u64>,
    /// The Up Next entry a click picked. A double click plays it.
    queue_picked: Option<u64>,
    /// The Now Playing or Up Next entry a right press opened the menu on.
    queue_menu: Option<(u64, TrackKey)>,
    /// A pick or sync that failed, as a headline and the plugin's reason.
    failure: Option<(SharedString, String)>,
    /// By row: the list only appends until a new place replaces it, which
    /// clears these.
    selected: HashSet<usize>,
    anchor: Option<usize>,
    cursor: Option<usize>,
    /// None means the press missed the rows, and the menu is the panel's own.
    menu_row: Option<usize>,
    search: gpui::Entity<SearchBox>,
    scroll: VirtualListScrollHandle,
    /// Each shelf's sideways scroll, by its first entry.
    shelves: HashMap<usize, gpui::ScrollHandle>,
    /// The list's width last laid out for. The dock caches panels, so a
    /// resize repaints without re-rendering; `rows` notifies on drift.
    width: Pixels,
    /// Drag-to-scroll on a shelf, and the shelf it's on by its first entry.
    /// A drag past its dead zone swallows the tile click.
    flick: FlickState,
    flicking: Option<usize>,
    flick_tick: Instant,
    /// Where the last search was typed, which clearing the box goes back to.
    searched_from: Option<Place>,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _library_changed: Subscription,
    _player_changed: Subscription,
    _selection_changed: Subscription,
    _thumbs_changed: Subscription,
    _search_events: Subscription,
}

impl SourceBrowserPanel {
    pub fn new(
        state: AppState,
        config: SourceBrowserConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Updated) {
                    this.library_changed(cx);
                }
            },
        );

        // The pump notifies every tick; only the playing and opening keys
        // matter here.
        let _player_changed = cx.observe(&state.player, |this: &mut Self, player, cx| {
            let player = player.read(cx);
            let playing = player.now_playing().map(|now| now.key);
            let opening = player.opening();
            let queue_rev = player.queue_rev();
            let audible = player.is_playing();

            if this.playing == playing
                && this.opening == opening
                && this.queue_rev == queue_rev
                && this.audible == audible
            {
                return;
            }

            if this.playing != playing || this.queue_rev != queue_rev {
                this.up_next = player.up_next(UP_NEXT);

                let picked = this.queue_picked;
                let now = player.playing_entry();
                if !picked.is_some_and(|picked| {
                    Some(picked) == now || this.up_next.iter().any(|(entry, _)| *entry == picked)
                }) {
                    this.queue_picked = None;
                }
            }

            this.playing = playing;
            this.opening = opening;
            this.queue_rev = queue_rev;
            this.audible = audible;
            cx.notify();
        });

        // Another panel's pick clears the marks here. The clear is local:
        // republishing would fight whoever just published.
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, event: &SelectionEvent, cx| {
                if event.source == cx.entity().entity_id() || this.selected.is_empty() {
                    return;
                }

                this.clear_marks();
                cx.notify();
            },
        );

        let _thumbs_changed = cx.observe(&state.thumbs, |_: &mut Self, _, cx| cx.notify());

        let search = cx.new(|cx| {
            SearchBox::new(rox_i18n::t!("source-browser-search"), "", window, cx)
                .small()
                .icon()
        });
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);

        let (playing, opening, audible) = {
            let player = state.player.read(cx);
            (
                player.now_playing().map(|now| now.key),
                player.opening(),
                player.is_playing(),
            )
        };

        let mut panel = SourceBrowserPanel {
            state,
            config,
            label: SharedString::default(),
            synced: Vec::new(),
            members: HashMap::new(),
            listing: Listing::default(),
            ids: HashMap::new(),
            syncing: HashSet::new(),
            flags_seen: 0,
            picking: HashSet::new(),
            playing,
            opening,
            audible,
            bars: PlayingBars::shared(),
            played_from: None,
            reveal_playing: false,
            fresh: None,
            up_next: Vec::new(),
            up_next_open: false,
            queue_rev: None,
            queue_picked: None,
            queue_menu: None,
            failure: None,
            selected: HashSet::new(),
            anchor: None,
            cursor: None,
            menu_row: None,
            search,
            scroll: VirtualListScrollHandle::new(),
            shelves: HashMap::new(),
            width: px(0.),
            flick: FlickState::default(),
            flicking: None,
            flick_tick: Instant::now(),
            searched_from: None,
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            _library_changed,
            _player_changed,
            _selection_changed,
            _thumbs_changed,
            _search_events,
        };

        if !panel.config.source.is_empty() {
            panel.read_record(cx);
            panel.go(Place::default(), cx);
        }

        panel
    }

    fn source(&self) -> &str {
        &self.config.source
    }

    /// Pins the panel to a source. The config is part of the layout dump, so
    /// the tab-panel repaint writes the choice to disk.
    fn choose(&mut self, source: String, cx: &mut Context<Self>) {
        self.config.source = source;
        self.listing = Listing::default();
        self.failure = None;
        self.clear_marks();

        self.read_record(cx);
        self.go(Place::default(), cx);
        panel::refresh_tab_panel(&self.tab_panel, cx);
    }

    /// The label, the synced collections and their row counts. Reads the
    /// settings file and the database, so never per frame.
    fn read_record(&mut self, cx: &App) {
        let record = self.source().strip_prefix(PLUGIN_PREFIX).and_then(|id| {
            Settings::load()
                .accounts
                .plugins
                .into_iter()
                .find(|record| record.id == id)
        });

        let live = plugins::live_sources()
            .into_iter()
            .find(|(source, _)| source == self.source())
            .map(|(_, label)| label);
        let label = live
            .or_else(|| record.as_ref().map(|record| record.label.clone()))
            .filter(|label| !label.is_empty())
            .unwrap_or_else(|| self.source().to_string());

        self.label = label.into();
        self.synced = record.map(|record| record.synced).unwrap_or_default();

        let conn = rox_library::store::open(&self.state.library.read(cx).db_path()).ok();
        self.members = match conn {
            Some(conn) => self
                .synced
                .iter()
                .map(|c| {
                    let rows = members::list(&conn, self.source(), &c.id).map_or(0, |k| k.len());
                    (c.id.clone(), rows)
                })
                .collect(),

            None => HashMap::new(),
        };
    }

    fn is_synced(&self, node: &str) -> bool {
        self.synced.iter().any(|c| c.id == node)
    }

    /// The node at `ix` as a kept collection stores it.
    fn kept_look(&self, ix: usize) -> Option<SyncedCollection> {
        let Some(Entry::Node {
            id,
            title,
            subtitle,
            kind,
            art,
            ..
        }) = self.listing.entries.get(ix)
        else {
            return None;
        };

        Some(SyncedCollection {
            id: id.clone(),
            title: title.clone(),
            subtitle: subtitle.clone(),
            kind: kind.map(NodeKind::name).unwrap_or_default().to_string(),
            art: art.clone(),
            token: String::new(),
        })
    }

    /// Kept collections in this listing whose look moved since they were
    /// stored, written back so the library draws them the same offline.
    fn restyle_kept(&mut self) {
        let seen: Vec<SyncedCollection> = (0..self.listing.entries.len())
            .filter_map(|ix| self.kept_look(ix))
            .filter(|look| {
                self.synced.iter().any(|kept| {
                    kept.id == look.id
                        && (&kept.title, &kept.subtitle, &kept.kind, &kept.art)
                            != (&look.title, &look.subtitle, &look.kind, &look.art)
                })
            })
            .collect();

        if seen.is_empty() {
            return;
        }

        for kept in self.synced.iter_mut() {
            if let Some(look) = seen.iter().find(|look| look.id == kept.id) {
                kept.title = look.title.clone();
                kept.subtitle = look.subtitle.clone();
                kept.kind = look.kind.clone();
                kept.art = look.art.clone();
            }
        }

        plugins::restyle_synced(self.source(), seen);
    }

    fn key_for(&self, key: &str) -> TrackKey {
        TrackKey {
            source: source_id(self.source()),
            path: key.into(),
            sub: 0,
        }
    }

    /// Lists a place from its first page. A synced collection lands at once
    /// from the library; everything else waits on the plugin.
    fn go(&mut self, mut place: Place, cx: &mut Context<Self>) {
        // A node the library doesn't keep, like one Go to names, opens from
        // the plugin as it would anywhere else.
        if place.library && place.node().is_some_and(|node| !self.is_synced(&node.id)) {
            place.library = false;
        }

        let generation = self.listing.begin(Some(place.clone()));

        if place.library && place.trail.is_empty() && place.query.is_none() {
            let page = self.library_home(cx);
            self.landed(generation, Ok(page), false, cx);
            return;
        }

        if let Some(node) = place.node()
            && self.is_synced(&node.id)
        {
            let page = self.library_page(&node.id, cx);
            self.landed(generation, Ok(page), false, cx);
            return;
        }

        let view = place.view.clone();
        let task = match &place.query {
            Some(query) => plugins::search(self.source(), query.clone(), view, None, cx),

            None => plugins::browse(
                self.source(),
                place.node().map(|node| node.id.clone()),
                view,
                None,
                cx,
            ),
        };

        self.await_page(generation, task, place.is_root(), cx);
        cx.notify();
    }

    fn load_more(&mut self, cx: &mut Context<Self>) {
        if !self.listing.wants_more() {
            return;
        }

        let (home, cursor, home_first) = self.listing.next_page();
        let place = self.listing.place.clone();
        let generation = self.listing.begin_page(None, home_first);

        // The roots' view means nothing to the home's page.
        let view = match home {
            Some(_) => None,
            None => place.view.clone(),
        };
        let task = match (&place.query, home) {
            (Some(query), _) => plugins::search(self.source(), query.clone(), view, cursor, cx),

            (None, Some(home)) => plugins::browse(self.source(), Some(home), view, cursor, cx),

            (None, None) => plugins::browse(
                self.source(),
                place.node().map(|node| node.id.clone()),
                view,
                cursor,
                cx,
            ),
        };

        self.await_page(generation, task, false, cx);
        cx.notify();
    }

    fn await_page(
        &mut self,
        generation: u64,
        task: Task<Result<Page, String>>,
        root: bool,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| this.landed(generation, result, root, cx))
                .ok();
        })
        .detach();
    }

    /// A root the plugin can't list falls back to the synced collections,
    /// which open without it.
    fn landed(
        &mut self,
        generation: u64,
        result: Result<Page, String>,
        root: bool,
        cx: &mut Context<Self>,
    ) {
        let before = self.listing.entries.len();
        let failed = result.is_err();
        let landed = match result {
            Err(e) if root => {
                let fallback = self.synced_page();
                let landed = self.listing.land(generation, Ok(fallback));
                if landed != Landed::Stale {
                    self.listing.error = Some(e);
                }
                landed
            }

            result => self.listing.land(generation, result),
        };

        if landed == Landed::Stale {
            return;
        }

        // A home following its roots eases in rather than popping. A re-read
        // of the place already shown doesn't fade, or every refresh flashes.
        match landed {
            Landed::Replaced { moved: true } => self.fresh = Some((0, Instant::now())),
            Landed::Appended => self.fresh = Some((before, Instant::now())),
            _ => {}
        }

        if failed {
            self.listing.unavailable = plugins::unavailable(self.source());
        } else {
            self.restyle_kept();
        }

        if matches!(landed, Landed::Replaced { .. })
            && !failed
            && self.config.home
            && self.listing.place.is_root()
        {
            self.listing.follow_home();
        }

        if landed == (Landed::Replaced { moved: true }) {
            self.clear_marks();
            self.shelves.clear();
            self.flicking = None;
            self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        }

        if landed == Landed::Appended && self.listing.sort.is_some() {
            self.listing.apply_sort();
            self.read_rest_for_sort(cx);
        }

        // Only the first page is looked at: a row further down stays where
        // it is rather than the list paging to it.
        if std::mem::take(&mut self.reveal_playing) && !failed {
            self.jump_to_playing(cx);
        }

        self.resolve_ids(cx);
        if !failed && !self.listing.place.library {
            self.keep_listed_go_to(cx);
        }
        cx.notify();
    }

    /// Sorts by a field, or back to the plugin's order with None, which asks
    /// for the place again. A sort only means something over the whole list,
    /// so the rest is read first, up to [`SORT_CAP`] rows.
    fn sort_by(&mut self, field: Option<String>, cx: &mut Context<Self>) {
        if field.is_none() {
            let place = self.listing.place.clone();
            self.go(place, cx);
            return;
        }

        self.listing.sort = field;
        self.listing.apply_sort();
        self.clear_marks();
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.read_rest_for_sort(cx);
        cx.notify();
    }

    fn read_rest_for_sort(&mut self, cx: &mut Context<Self>) {
        if self.listing.entries.len() < SORT_CAP {
            self.load_more(cx);
        }
    }

    /// Everything a node lists, into the queue the way `how` says.
    fn play_node(&mut self, node: String, how: NodePlay, cx: &mut Context<Self>) {
        self.failure = None;
        self.picking.insert(node.clone());
        cx.notify();

        // The place shown, when it's the node, or the node a row opens.
        let from = match self.listing.place.node() {
            Some(crumb) if crumb.id == node => Some(self.listing.place.clone()),
            _ => self
                .listing
                .entries
                .iter()
                .position(|entry| matches!(entry, Entry::Node { id, .. } if *id == node))
                .and_then(|ix| self.opened(ix)),
        }
        .unwrap_or_else(|| self.listing.place.clone());

        let task = plugins::node_tracks(self.source(), &node, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.picking.remove(&node);

                match result {
                    Ok(tracks) if !tracks.is_empty() => match how {
                        NodePlay::Play => this.play(tracks, 0, from, cx),
                        NodePlay::Next => this.queue(tracks, true, cx),
                        NodePlay::Queue => this.queue(tracks, false, cx),
                    },

                    Ok(_) => {
                        this.failure = Some((
                            rox_i18n::t!("source-browser-pick-failed"),
                            rox_i18n::t!("source-browser-node-empty").to_string(),
                        ));
                    }

                    Err(e) => this.failure = Some((rox_i18n::t!("source-browser-pick-failed"), e)),
                }

                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Plays the plugin's station from a row: the row's own tracks first,
    /// then the station. `busy` is the row's key or id, spinning meanwhile.
    /// A failure lands after the menu has closed, so it's a toast on
    /// `origin`.
    fn play_similar(
        &mut self,
        seed: RadioSeed,
        busy: String,
        title: String,
        origin: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) {
        self.picking.insert(busy.clone());
        cx.notify();

        let task = plugins::play_similar(
            self.state.library.clone(),
            self.state.player.clone(),
            self.source(),
            seed,
            cx,
        );

        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.picking.remove(&busy);
                if let Err(e) = result {
                    Toast::new(Tone::Bad, e)
                        .title(rox_i18n::t!("source-browser-similar-failed", title = title))
                        .post(origin, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn synced_page(&self) -> Page {
        let entries = self
            .synced
            .iter()
            .map(|c| Entry::Node {
                id: c.id.clone(),
                title: c.title.clone(),
                // The row adds the library's count, as it does for any kept node.
                subtitle: c.subtitle.clone(),
                collection: true,
                kind: NodeKind::named(&c.kind),
                art: c.art.clone(),
                home: false,
            })
            .collect();

        Page {
            entries,
            ..Page::default()
        }
    }

    /// The plugin's part of the library: the kept collections, then the
    /// tracks added one at a time. Nothing here asks the plugin, so it lists
    /// the same with the plugin stopped.
    fn library_home(&self, cx: &App) -> Page {
        let saved = rox_library::store::open(&self.state.library.read(cx).db_path())
            .ok()
            .and_then(|conn| members::saved(&conn, self.source()).ok())
            .unwrap_or_default();

        let mut entries = Vec::new();

        if !self.synced.is_empty() {
            entries.push(Entry::Section {
                title: rox_i18n::t!("source-browser-library-kept").to_string(),
                tiles: false,
            });
            entries.extend(self.synced_page().entries);
        }

        let tracks = self.track_entries(&saved, cx);
        if !tracks.is_empty() {
            entries.push(Entry::Section {
                title: rox_i18n::t!("source-browser-library-added").to_string(),
                tiles: false,
            });
            entries.extend(tracks);
        }

        let notice = entries.is_empty().then(|| Notice {
            text: rox_i18n::t!(
                "source-browser-library-empty",
                source = self.label.to_string()
            )
            .to_string(),
            setup: false,
            link: None,
        });

        Page {
            entries,
            notice,
            go_to: self.library_go_to(&saved, cx),
            ..Page::default()
        }
    }

    /// A synced collection's tracks as the library holds them, in the
    /// plugin's order.
    fn library_page(&self, node: &str, cx: &App) -> Page {
        let keys = rox_library::store::open(&self.state.library.read(cx).db_path())
            .ok()
            .and_then(|conn| members::list(&conn, self.source(), node).ok())
            .unwrap_or_default();

        Page {
            entries: self.track_entries(&keys, cx),
            go_to: self.library_go_to(&keys, cx),
            ..Page::default()
        }
    }

    /// Go to for library rows, as the plugin last gave it with them.
    fn library_go_to(&self, keys: &[TrackKey], cx: &App) -> HashMap<String, GoTo> {
        let paths: Vec<String> = keys
            .iter()
            .map(|key| key.path.to_string_lossy().into_owned())
            .collect();

        rox_library::store::open(&self.state.library.read(cx).db_path())
            .ok()
            .and_then(|conn| members::go_to(&conn, self.source(), &paths).ok())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(key, stored)| Some((key, plugins::go_to_from(&stored)?)))
            .collect()
    }

    /// Rows for library tracks, from the tags the library holds.
    fn track_entries(&self, keys: &[TrackKey], cx: &App) -> Vec<Entry> {
        let library = self.state.library.read(cx);

        keys.iter()
            .filter_map(|key| {
                let (_, meta) = library.resolve_key(key)?;
                Some(Entry::Track(PluginTrack {
                    key: key.path.to_string_lossy().into_owned(),
                    title: meta.title,
                    artist: meta.artist,
                    album_artist: meta.album_artist,
                    album: meta.album,
                    genre: meta.genre,
                    year: meta.year,
                    track_no: meta.track_no,
                    duration_ms: meta.duration_ms,
                    codec: meta.codec,
                    bitrate_kbps: meta.bitrate_kbps,
                    ..PluginTrack::default()
                }))
            })
            .collect()
    }

    fn resolve_ids(&mut self, cx: &App) {
        let library = self.state.library.read(cx);
        let source = source_id(self.source());

        self.ids = self
            .listing
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Track(track) => {
                    let key = TrackKey {
                        source: source.clone(),
                        path: track.key.clone().into(),
                        sub: 0,
                    };
                    library.id_for_key(&key).map(|id| (track.key.clone(), id))
                }

                Entry::Node { .. } | Entry::Section { .. } => None,
            })
            .collect();
    }

    /// A synced collection being shown re-reads, since its rows may have
    /// just moved; anything the plugin listed stays as it was.
    fn library_changed(&mut self, cx: &mut Context<Self>) {
        if self.source().is_empty() {
            return;
        }

        self.read_record(cx);
        self.resolve_ids(cx);
        if self.retry_if_back(cx) {
            cx.notify();
            return;
        }

        // Anything the library lists is read again: a removal or a sync
        // elsewhere shows here at once.
        let place = self.listing.place.clone();
        let shows_synced = place.node().is_some_and(|node| self.is_synced(&node.id));
        if (shows_synced || place.library) && !self.listing.loading() {
            self.go(place, cx);
        }

        cx.notify();
    }

    /// Asks again for the place shown when the source that couldn't answer
    /// is back. Answers whether it asked.
    fn retry_if_back(&mut self, cx: &mut Context<Self>) -> bool {
        let back = self.listing.unavailable.is_some()
            && !self.listing.loading()
            && plugins::answers(self.source());
        if !back {
            return false;
        }

        let place = self.listing.place.clone();
        self.go(place, cx);

        true
    }

    fn set_synced(&mut self, kept: SyncedCollection, on: bool, cx: &mut Context<Self>) {
        let (node, title) = (kept.id.clone(), kept.title.clone());
        let task = plugins::set_synced(self.state.library.clone(), self.source(), kept, on, cx);

        // The record is written before the task starts, so the switch reads
        // the new state now and spins until the rows follow.
        self.read_record(cx);
        self.syncing.insert(node.clone());
        self.failure = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.syncing.remove(&node);

                if let Err(e) = result {
                    this.failure =
                        Some((rox_i18n::t!("source-browser-sync-failed", title = title), e));
                }

                this.read_record(cx);

                // Inside the collection, the list swaps between the plugin's
                // and the library's.
                let place = this.listing.place.clone();
                if place.node().is_some_and(|crumb| crumb.id == node) {
                    this.go(place, cx);
                }

                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Hands `then` the tracks' keys once every one is a library row. Tracks
    /// that already are skip the pick. Only `busy` spins while it runs: a
    /// play picks the rows around the one clicked, and those weren't asked for.
    fn with_picked(
        &mut self,
        tracks: Vec<PluginTrack>,
        busy: Option<String>,
        then: impl FnOnce(Vec<TrackKey>, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) {
        let keys: Vec<TrackKey> = tracks.iter().map(|t| self.key_for(&t.key)).collect();
        let unpicked: Vec<PluginTrack> = tracks
            .into_iter()
            .filter(|t| !self.ids.contains_key(&t.key))
            .collect();

        if unpicked.is_empty() {
            then(keys, cx);
            return;
        }

        let marked: Vec<String> = match busy {
            Some(key) => vec![key],
            None => unpicked.iter().map(|t| t.key.clone()).collect(),
        };
        self.picking.extend(marked.iter().cloned());
        self.failure = None;
        cx.notify();

        let task = plugins::pick(self.state.library.clone(), self.source(), unpicked, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                for key in &marked {
                    this.picking.remove(key);
                }

                match result {
                    Ok(_) => {
                        this.resolve_ids(cx);
                        then(keys, cx);
                    }

                    Err(e) => {
                        this.failure = Some((rox_i18n::t!("source-browser-pick-failed"), e));
                    }
                }

                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The tracks become the playing context, as a library run does, so
    /// playback carries on through them (ADR 16).
    /// Plays from the tracks, and goes on with the plugin's radio when they
    /// run out, where it has one. `from` is the place Home leads back to
    /// while these tracks play.
    fn play(
        &mut self,
        tracks: Vec<PluginTrack>,
        start: usize,
        from: Place,
        cx: &mut Context<Self>,
    ) {
        self.played_from = Some(PlayedFrom {
            place: from,
            keys: tracks.iter().map(|track| track.key.clone()).collect(),
        });

        let player = self.state.player.clone();
        let source = self.source().to_string();
        let busy = tracks.get(start).map(|track| track.key.clone());
        self.with_picked(
            tracks,
            busy,
            move |keys, cx| {
                let follow = plugins::follow_on(&source, cx);
                player.update(cx, |player, cx| {
                    player.play_at(keys, start, cx);
                    // After the play: starting a session clears the scope.
                    if let Some(scope) = follow {
                        player.set_scope(scope);
                    }
                })
            },
            cx,
        );
    }

    /// The place's own tracks, into the queue the way `how` says: in the
    /// order shown when the whole list is here, a sort included, and read
    /// from the plugin otherwise.
    fn play_place(&mut self, how: NodePlay, cx: &mut Context<Self>) {
        let Some(node) = self.listing.place.node().map(|node| node.id.clone()) else {
            return;
        };

        if self.listing.unread() {
            self.play_node(node, how, cx);
            return;
        }

        let rows: Vec<usize> = (0..self.listing.entries.len()).collect();
        let tracks = self.tracks_at(&rows);
        match how {
            NodePlay::Play => self.play(tracks, 0, self.listing.place.clone(), cx),
            NodePlay::Next => self.queue(tracks, true, cx),
            NodePlay::Queue => self.queue(tracks, false, cx),
        }
    }

    /// Play, Play Next, Add to Queue and Add to Library for the place shown,
    /// when it's a node that lists tracks: an album, a playlist, a mix.
    fn place_actions(&self, cx: &mut Context<Self>) -> Option<Div> {
        let node = self.listing.place.node()?;
        if !self
            .listing
            .entries
            .iter()
            .any(|entry| matches!(entry, Entry::Track(_)))
        {
            return None;
        }

        let busy = self.picking.contains(&node.id);
        let panel = cx.entity().downgrade();
        let button = |label: SharedString, icon: &'static str, how: NodePlay| {
            let panel = panel.clone();
            crate::settings::ui::small_button(label, icon, busy, move |_, _, cx| {
                panel.update(cx, |this, cx| this.play_place(how, cx)).ok();
            })
        };

        let keep = node.collection.then(|| self.keep_action(node, cx));

        Some(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(tokens::SPACE_XS)
                .child(button(
                    rox_i18n::t!("source-browser-play"),
                    icons::PLAY,
                    NodePlay::Play,
                ))
                .child(button(
                    rox_i18n::t!("panel-play-next"),
                    icons::SKIP_FORWARD,
                    NodePlay::Next,
                ))
                .child(button(
                    rox_i18n::t!("panel-add-to-queue"),
                    icons::LIST_MUSIC,
                    NodePlay::Queue,
                ))
                .children(keep),
        )
    }

    /// The listed row of the playing track, on the pages read so far.
    fn playing_row(&self) -> Option<usize> {
        let playing = self.playing.as_ref()?;
        if playing.source.as_ref() != self.source() {
            return None;
        }

        self.listing.entries.iter().position(|entry| {
            matches!(entry, Entry::Track(track) if playing.path.as_os_str() == track.key.as_str())
        })
    }

    /// Picks the playing row and scrolls it to the middle, the History
    /// panel's way.
    fn jump_to_playing(&mut self, cx: &mut Context<Self>) {
        let Some(row) = self.playing_row() else {
            return;
        };

        self.select(row, Modifiers::default(), cx);
        if let Some(item) = visual_of(&layout(&self.listing.entries, self.columns()), row) {
            self.scroll.scroll_to_item(item, ScrollStrategy::Center);
        }
        cx.notify();
    }

    /// Opens the place the playing track was played from and finds its row
    /// there once the list lands.
    fn back_to_playing(&mut self, cx: &mut Context<Self>) {
        let Some(from) = self.played_from.as_ref() else {
            return;
        };

        self.reveal_playing = true;
        self.go(from.place.clone(), cx);
    }

    /// The place's keep switch, the row button's twin for someone already
    /// inside the collection.
    fn keep_action(&self, node: &Crumb, cx: &mut Context<Self>) -> SmallButton {
        let synced = self.is_synced(&node.id);
        let busy = self.syncing.contains(&node.id);

        let (label, icon) = match synced {
            true => (
                rox_i18n::t!("source-browser-remove-from-library"),
                icons::MINUS,
            ),
            false => (rox_i18n::t!("source-browser-add-to-library"), icons::PLUS),
        };

        // Inside the collection there's no row to read its line and art
        // from; the next listing that shows it fills them in.
        let kept = SyncedCollection {
            id: node.id.clone(),
            title: node.title.clone(),
            kind: node
                .kind
                .map(NodeKind::name)
                .unwrap_or_default()
                .to_string(),
            ..SyncedCollection::default()
        };

        let panel = cx.entity().downgrade();
        crate::settings::ui::small_button(label, icon, busy, move |_, _, cx| {
            let kept = kept.clone();
            panel
                .update(cx, |this, cx| {
                    if !this.syncing.contains(&kept.id) {
                        this.set_synced(kept, !synced, cx);
                    }
                })
                .ok();
        })
    }

    /// The listed tracks around a row, from it onward. Every one is picked
    /// first, since only rows can be queued.
    fn play_from(&mut self, ix: usize, cx: &mut Context<Self>) {
        let rows: Vec<usize> = (0..self.listing.entries.len())
            .filter(|&row| self.track_at(row).is_some())
            .collect();
        let Some(at) = rows.iter().position(|&row| row == ix) else {
            return;
        };

        let (window, start) = play_window(rows.len(), at, QUEUE_CAP);
        let tracks = self.tracks_at(&rows[window]);
        self.play(tracks, start, self.listing.place.clone(), cx);
    }

    fn queue(&mut self, tracks: Vec<PluginTrack>, next: bool, cx: &mut Context<Self>) {
        let player = self.state.player.clone();
        self.with_picked(
            tracks,
            None,
            move |keys, cx| {
                player.update(cx, |player, cx| match next {
                    true => player.play_next(keys, cx),
                    false => player.enqueue(keys, cx),
                })
            },
            cx,
        );
    }

    fn track_at(&self, ix: usize) -> Option<&PluginTrack> {
        match self.listing.entries.get(ix)? {
            Entry::Track(track) => Some(track),
            Entry::Node { .. } | Entry::Section { .. } => None,
        }
    }

    fn tracks_at(&self, rows: &[usize]) -> Vec<PluginTrack> {
        rows.iter()
            .filter_map(|&ix| self.track_at(ix).cloned())
            .collect()
    }

    fn selected_rows(&self) -> Vec<usize> {
        let mut rows: Vec<usize> = self.selected.iter().copied().collect();
        rows.sort_unstable();
        rows
    }

    /// A node opens; a track plays.
    fn activate(&mut self, ix: usize, cx: &mut Context<Self>) {
        match self.listing.entries.get(ix) {
            Some(Entry::Node { .. }) => {
                if let Some(place) = self.opened(ix) {
                    self.go(place, cx);
                }
            }

            Some(Entry::Track(_)) => self.play_from(ix, cx),

            Some(Entry::Section { .. }) | None => {}
        }
    }

    /// Where opening the node at `ix` goes.
    fn opened(&self, ix: usize) -> Option<Place> {
        let Some(Entry::Node {
            id,
            title,
            collection,
            kind,
            ..
        }) = self.listing.entries.get(ix)
        else {
            return None;
        };

        Some(self.listing.place.clone().opening(Crumb {
            id: id.clone(),
            title: title.clone(),
            query: None,
            collection: *collection,
            kind: *kind,
        }))
    }

    fn gone_to(&self, target: &Target) -> Place {
        self.listing.place.clone().going_to(target)
    }

    /// Opens a node Go to named, from wherever the panel is.
    pub fn open_target(&mut self, target: &Target, cx: &mut Context<Self>) {
        let place = self.gone_to(target);
        self.go(place, cx);
    }

    /// Keeps the first `depth` crumbs. The crumb being shown reloads, which
    /// is how a failed page is asked for again.
    fn go_up(&mut self, depth: usize, cx: &mut Context<Self>) {
        let mut trail = self.listing.place.trail.clone();
        trail.truncate(depth);

        // Landing on a results crumb lists the results again.
        let query = match trail.last().is_some_and(|crumb| crumb.query.is_some()) {
            true => trail.pop().and_then(|crumb| crumb.query),
            false => None,
        };

        let library = self.listing.place.library;
        self.go(
            Place {
                trail,
                query,
                view: None,
                library,
            },
            cx,
        );
    }

    fn on_search_event(
        &mut self,
        search: &gpui::Entity<SearchBox>,
        event: &SearchEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // A search is a request to the plugin's service, so it runs on
            // Enter rather than per keystroke. Clearing the box goes back.
            SearchEvent::Submitted => {
                let query = search.read(cx).query().trim().to_string();
                if query.is_empty() || self.source().is_empty() {
                    return;
                }

                // A search covers the whole service, so its results sit
                // right under Home rather than under the place it was typed.
                // A search over results goes back to where the first began.
                let from = self.listing.target().clone();
                if from.query.is_none() {
                    self.searched_from = Some(from);
                }

                self.go(
                    Place {
                        query: Some(query),
                        ..Place::default()
                    },
                    cx,
                );
            }

            // The box keeps its text when a crumb or a result navigates away,
            // so only a clear over results goes back.
            SearchEvent::Changed => {
                let cleared = search.read(cx).query().trim().is_empty();
                if cleared && self.listing.target().query.is_some() {
                    let back = self.searched_from.take().unwrap_or_default();
                    self.go(back, cx);
                }
            }

            SearchEvent::Dismissed => window.focus(&self.focus),

            SearchEvent::FocusChanged => cx.notify(),
        }
    }

    fn clear_marks(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.cursor = None;
    }

    fn select(&mut self, ix: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        if ix >= self.listing.entries.len() {
            return;
        }

        self.queue_picked = None;

        if modifiers.shift {
            let anchor = self.anchor.unwrap_or(ix);
            let range = anchor.min(ix)..=anchor.max(ix);
            // Ctrl+shift stacks the range onto the set.
            if modifiers.secondary() {
                self.selected.extend(range);
            } else {
                self.selected = range.collect();
            }
            if self.anchor.is_none() {
                self.anchor = Some(ix);
            }
        } else if modifiers.secondary() {
            if !self.selected.insert(ix) {
                self.selected.remove(&ix);
            }
            self.anchor = Some(ix);
        } else {
            self.selected = HashSet::from([ix]);
            self.anchor = Some(ix);
        }

        self.cursor = Some(ix);
        self.publish_selection(cx);
        cx.notify();
    }

    /// Every call publishes, an empty set included, so a pick here replaces
    /// one made elsewhere. Only tracks that are rows have an id to publish.
    fn publish_selection(&self, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self
            .selected_rows()
            .into_iter()
            .filter_map(|ix| self.track_at(ix))
            .filter_map(|track| self.ids.get(&track.key).copied())
            .collect();
        let source = cx.entity_id();

        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, source, cx));
    }

    fn step(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        let len = self.listing.entries.len();
        if len == 0 {
            return;
        }

        let target = match self.cursor {
            Some(ix) => ix.saturating_add_signed(delta).min(len - 1),
            None => 0,
        };

        self.select(
            target,
            Modifiers {
                shift: extend,
                ..Modifiers::default()
            },
            cx,
        );
        if let Some(item) = visual_of(&layout(&self.listing.entries, self.columns()), target) {
            self.scroll.scroll_to_item(item, ScrollStrategy::Center);
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &Window, cx: &mut Context<Self>) {
        // The search box's own keys bubble up through the panel; its Enter
        // searches and must not play the selection too.
        if self.search.read(cx).is_focused(window, cx) {
            return;
        }

        let modifiers = &event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();

        if modifiers.secondary() && key == "a" {
            self.selected = (0..self.listing.entries.len()).collect();
            self.anchor = Some(0);
            self.cursor = Some(0);
            self.publish_selection(cx);
            cx.notify();
            return;
        }

        match key {
            "up" => self.step(-1, modifiers.shift, cx),

            "down" => self.step(1, modifiers.shift, cx),

            // One node opens; any tracks in the set play, nodes skipped.
            "enter" => {
                let rows = self.selected_rows();
                match rows.as_slice() {
                    [ix] => self.activate(*ix, cx),

                    rows => {
                        let tracks = self.tracks_at(rows);
                        if !tracks.is_empty() {
                            self.play(tracks, 0, self.listing.place.clone(), cx);
                        }
                    }
                }
            }

            "escape" if !self.selected.is_empty() => {
                self.clear_marks();
                self.publish_selection(cx);
                cx.notify();
            }

            _ => {}
        }
    }

    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let root =
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(palette::bg_root())
                .track_focus(&self.focus)
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    this.on_key(event, window, cx)
                }));

        if self.source().is_empty() {
            return root.child(self.picker(cx));
        }

        // Switching a plugin on from Settings redraws this window.
        self.retry_if_back(cx);

        // An action's toast redraws it too, with the flags it reported.
        if let Some((seen, flags)) =
            rox_services::plugin_actions::flags_since(self.source(), self.flags_seen)
        {
            self.flags_seen = seen;
            self.listing.flags.extend(flags);
        }

        let failed = match self.listing.unavailable {
            Some(reason) => Some(self.unavailable_banner(reason)),

            None => self.listing.error.clone().map(|reason| {
                panel::banner(
                    Tone::Bad,
                    rox_i18n::t!("source-browser-failed", source = self.label.to_string()),
                    vec![reason.into()],
                )
            }),
        };
        let picked = self
            .failure
            .clone()
            .map(|(headline, reason)| panel::banner(Tone::Bad, headline, vec![reason.into()]));
        let notice = self.listing.notice.as_ref().map(notice_banner);

        let view = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.breadcrumb(cx))
            .children(self.views_bar(cx))
            .children(self.fields_bar(cx))
            .children(
                [failed, picked, notice]
                    .into_iter()
                    .flatten()
                    .map(|banner| div().flex_none().p(tokens::SPACE_SM).child(banner)),
            )
            // Its own menu: the list's reads rows of the listing, not the queue.
            // Every context menu takes the same element id, so this one sits
            // under an id of its own, or it shares the list's open state and
            // neither menu's clicks land.
            .children(self.now_playing(cx).map(|block| {
                let weak = cx.entity().downgrade();
                div()
                    .id("source-queue-menu")
                    .flex_none()
                    .child(block.context_menu(move |menu, window, cx| {
                        let Some(this) = weak.upgrade() else {
                            return menu;
                        };
                        this.update(cx, |this, cx| this.queue_row_menu(menu, window, cx))
                    }))
            }))
            // One menu over the whole list. A menu per row shares one element
            // state across the rows and never opens.
            .child(self.list(cx).context_menu({
                let weak = cx.entity().downgrade();
                move |menu, window, cx| {
                    let Some(this) = weak.upgrade() else {
                        return menu;
                    };
                    this.update(cx, |this, cx| this.row_menu(menu, window, cx))
                }
            }));

        root.child(self.header(cx)).child(self.loading_veil(view))
    }

    /// While a new place loads, the old one dims under a spinner rather than
    /// a spinner squeezing the header. The dim eases in, so an answer that
    /// lands fast barely shows it. The next page of the same place only gets
    /// a spinner over the list's foot.
    fn loading_veil(&self, view: Div) -> Div {
        let spinner = || Spinner::new().color(palette::accent().into());

        let shell = div().flex_1().min_h_0().relative().flex().flex_col();

        let Some(generation) = self.listing.navigating() else {
            let paging = self.listing.loading().then(|| {
                div()
                    .absolute()
                    .bottom(tokens::SPACE_MD)
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(spinner().small())
            });

            return shell.child(view).children(paging);
        };

        let dimmed = view.with_animation(
            ElementId::NamedInteger("source-veil".into(), generation),
            Animation::new(Duration::from_secs_f32(tokens::EASE_SECS))
                .with_easing(ease_out_quint()),
            |view, delta| view.opacity(1. - VEIL * delta),
        );

        shell.child(dimmed).child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .child(spinner()),
        )
    }

    fn header(&mut self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .border_b_1()
            .border_color(palette::border())
            .child(self.source_mark(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(self.search.update(cx, |search, cx| search.element(cx))),
            )
            .child(self.library_toggle(cx))
    }

    /// The way into what rox holds of this source, opposite the mark that
    /// leads to the plugin's home. Pressed again, it goes back home.
    fn library_toggle(&self, cx: &mut Context<Self>) -> SmallButton {
        let inside = self.listing.place.library;

        crate::settings::ui::small_button(
            rox_i18n::t!("source-browser-library"),
            icons::LIST_MUSIC,
            false,
            cx.listener(move |this, _, _, cx| {
                let to = match inside {
                    true => Place::default(),
                    false => Place::library(),
                };
                this.go(to, cx);
            }),
        )
        .keyed("source-library")
        .when(inside, |button| {
            button
                .bg(palette::alpha(palette::accent(), 0x26))
                .text_color(palette::text_bright())
        })
    }

    /// The plugin's icon where it ships one, named in a tooltip, or its label.
    /// A click lists the roots afresh, the one way back to them from the roots
    /// themselves, where there's no crumb.
    fn source_mark(&self, cx: &mut Context<Self>) -> AnyElement {
        let mark = div()
            .id("source-mark")
            .flex_none()
            .cursor_pointer()
            .on_click(
                cx.listener(|this, _: &gpui::ClickEvent, _, cx| this.go(Place::default(), cx)),
            );

        let Some(icon) = plugins::icon(self.source()) else {
            return mark
                .max_w(px(200.))
                .truncate()
                .text_sm()
                .text_color(palette::text_bright())
                .hover(|mark| mark.text_color(palette::accent()))
                .child(self.label.clone())
                .into_any_element();
        };

        let label = self.label.clone();
        mark.tooltip(move |window, cx| {
            gpui_component::tooltip::Tooltip::new(label.clone()).build(window, cx)
        })
        .child(
            svg()
                .path(icon)
                .size(px(18.))
                .text_color(palette::text_bright()),
        )
        .into_any_element()
    }

    /// The other ways the plugin can list the place shown.
    fn views_bar(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.listing.views.is_empty() {
            return None;
        }

        let chips = self.listing.views.iter().enumerate().map(|(ix, view)| {
            let on = self.listing.view.as_deref() == Some(view.id.as_str());
            let id = view.id.clone();

            div()
                .id(("source-view", ix))
                .px(tokens::SPACE_SM)
                .py(px(2.))
                .rounded_full()
                .text_xs()
                .bg(match on {
                    true => palette::alpha(palette::accent(), 0x26),
                    false => palette::bg_control(),
                })
                .text_color(match on {
                    true => palette::text_bright(),
                    false => palette::text_secondary(),
                })
                .when(!on, |chip| {
                    chip.cursor_pointer()
                        .hover(|chip| chip.text_color(palette::text_bright()))
                        .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                            this.pick_view(id.clone(), cx)
                        }))
                })
                .child(SharedString::from(view.label.clone()))
        });

        Some(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap(tokens::SPACE_XS)
                .px(tokens::SPACE_SM)
                .py(tokens::SPACE_XS)
                .border_b_1()
                .border_color(palette::border())
                .children(chips),
        )
    }

    /// Column headings for the plugin's fields, lined up with the rows' cells
    /// at their right edge. A heading sorts by its field; the sorted one
    /// again goes back to the plugin's order.
    fn fields_bar(&self, cx: &mut Context<Self>) -> Option<Div> {
        let actions = self.place_actions(cx);
        if self.listing.fields.is_empty() && actions.is_none() {
            return None;
        }

        let capped = self.listing.sort.is_some()
            && self.listing.unread()
            && self.listing.entries.len() >= SORT_CAP;

        let headings = self.listing.fields.iter().enumerate().map(|(ix, field)| {
            let on = self.listing.sort.as_deref() == Some(field.id.as_str());
            let next = (!on).then(|| field.id.clone());

            div()
                .id(("source-field", ix))
                .flex_none()
                .w(FIELD_W)
                .flex()
                .justify_end()
                .truncate()
                .cursor_pointer()
                .text_color(match on {
                    true => palette::accent(),
                    false => palette::text_muted(),
                })
                .hover(|heading| heading.text_color(palette::text_bright()))
                .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    this.sort_by(next.clone(), cx)
                }))
                .child(SharedString::from(field.label.clone()))
        });

        Some(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_SM)
                .px(tokens::SPACE_SM)
                .py(tokens::SPACE_XS)
                .text_xs()
                .border_b_1()
                .border_color(palette::border())
                .children(actions)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(palette::text_faint())
                        .when(capped, |line| {
                            line.child(rox_i18n::t!(
                                "source-browser-sorted-first",
                                count = SORT_CAP as u64
                            ))
                        }),
                )
                .children(headings),
        )
    }

    /// A row's field values, formatted for their columns.
    fn field_cells(&self, ix: usize) -> Vec<AnyElement> {
        let row = self.listing.values.get(ix);

        self.listing
            .fields
            .iter()
            .map(|field| {
                let text = row
                    .and_then(|row| value_of(row, &field.id))
                    .map(|value| fmt_field(field.kind, value))
                    .unwrap_or_default();

                div()
                    .flex_none()
                    .w(FIELD_W)
                    .flex()
                    .justify_end()
                    .truncate()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(SharedString::from(text))
                    .into_any_element()
            })
            .collect()
    }

    /// What's playing and what's next, over the roots while this source's
    /// tracks are playing or coming up: a shuffled or radio queue says here
    /// what it will do. It stays while a track from elsewhere plays between
    /// them, so the block doesn't vanish on a jump.
    fn now_playing(&self, cx: &mut Context<Self>) -> Option<Div> {
        let playing = self.playing.clone()?;
        let ours = |key: &TrackKey| key.source.as_ref() == self.source();
        if !self.listing.place.is_root()
            || !(ours(&playing) || self.up_next.iter().any(|(_, key)| ours(key)))
        {
            return None;
        }

        let heading = |text: SharedString| {
            div()
                .px(tokens::SPACE_SM)
                .pt(tokens::SPACE_SM)
                .pb(tokens::SPACE_XS)
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(palette::text_muted())
                .child(text)
        };

        let now = self.state.player.read(cx).playing_entry();
        let back = self.back_link(&playing, cx);
        let title = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(heading(rox_i18n::t!("source-browser-now-playing")))
            .children(back);

        let block = div()
            .flex_none()
            .flex()
            .flex_col()
            .border_b_1()
            .border_color(palette::border())
            .child(title)
            .child(self.queue_row("source-now", 0, &playing, now, true, cx));

        if self.up_next.is_empty() {
            return Some(block);
        }

        let toggle = self.up_next_toggle(cx);
        if !self.up_next_open {
            return Some(block.child(self.up_next_line(toggle, cx)));
        }

        let next = self
            .up_next
            .iter()
            .enumerate()
            .map(|(ix, (entry, key))| {
                self.queue_row("source-next", ix, key, Some(*entry), false, cx)
            })
            .collect::<Vec<_>>();

        let title = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .pr(tokens::SPACE_XS)
            .child(heading(rox_i18n::t!("source-browser-up-next")))
            .child(toggle);

        Some(block.child(title).children(next))
    }

    /// Folds Up Next to its first track or opens it to all of them.
    fn up_next_toggle(&self, cx: &mut Context<Self>) -> IconButton {
        let icon = match self.up_next_open {
            true => icons::CHEVRON_UP,
            false => icons::CHEVRON_DOWN,
        };

        let panel = cx.entity().downgrade();
        icon_button(icon, false, move |_, _, cx| {
            panel
                .update(cx, |this, cx| {
                    this.up_next_open = !this.up_next_open;
                    cx.notify();
                })
                .ok();
        })
        .keyed("source-up-next-toggle")
    }

    /// Folded Up Next: the first track's title and artist on one line, with
    /// the toggle outside the row so pressing it doesn't pick the track.
    fn up_next_line(&self, toggle: IconButton, cx: &mut Context<Self>) -> Div {
        let (entry, key) = self.up_next[0].clone();
        let (title, artist) = self.queue_tags(&key, cx);
        let art = self.art_sized(&self.queue_art_key(&key), icons::MUSIC, UP_NEXT_ART, cx);
        let picked = Some(entry) == self.queue_picked;

        let row = div()
            .id(("source-next", 0usize))
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .pl(tokens::SPACE_SM)
            .text_xs()
            .when(picked, |row| {
                row.bg(palette::alpha(palette::accent(), 0x26))
            })
            .child(
                div()
                    .flex_none()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("source-browser-up-next")),
            )
            .child(art)
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(palette::text())
                    .child(SharedString::from(title)),
            )
            .when(!artist.is_empty(), |row| {
                row.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(palette::text_muted())
                        .child(SharedString::from(artist)),
                )
            });

        div()
            .flex_none()
            .h(UP_NEXT_LINE_H)
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .pr(tokens::SPACE_XS)
            .child(self.queue_presses(row, &key, Some(entry), cx))
            .child(toggle)
    }

    /// The place the playing track was played from, named the way its crumb
    /// reads, while it's one of the tracks that play sent.
    fn back_link(&self, playing: &TrackKey, cx: &mut Context<Self>) -> Option<AnyElement> {
        let from = self.played_from.as_ref()?;
        if playing.source.as_ref() != self.source()
            || !from.keys.contains(playing.path.to_string_lossy().as_ref())
        {
            return None;
        }

        let name: SharedString = match (&from.place.query, from.place.trail.last()) {
            (Some(query), _) => rox_i18n::t!("source-browser-results", query = query.clone()),
            (None, Some(crumb)) => crumb.title.clone().into(),
            (None, None) => return None,
        };

        Some(
            div()
                .id("source-back-to-playing")
                .flex()
                .flex_row()
                .items_center()
                .gap(px(2.))
                .min_w_0()
                .mx(tokens::SPACE_SM)
                .mt(tokens::SPACE_SM)
                .mb(tokens::SPACE_XS)
                .px(tokens::SPACE_XS)
                .rounded(tokens::RADIUS)
                .cursor_pointer()
                .text_xs()
                .text_color(palette::text_muted())
                .hover(|link| {
                    link.bg(palette::bg_control_hover())
                        .text_color(palette::text())
                })
                .on_click(cx.listener(|this, _: &gpui::ClickEvent, _, cx| this.back_to_playing(cx)))
                .child(div().min_w_0().truncate().child(name))
                .child(
                    svg()
                        .flex_none()
                        .path(icons::CHEVRON_RIGHT)
                        .size(px(12.))
                        .text_color(palette::text_muted()),
                )
                .into_any_element(),
        )
    }

    /// The bars over a cover while its track is the one playing.
    fn playing_art(&self, key: &str, cx: &mut Context<Self>) -> AnyElement {
        let feed = self.state.player.read(cx).feed();
        div()
            .relative()
            .flex_none()
            .size(ART)
            .child(self.art(key, icons::MUSIC, cx))
            .child(playing_bars::overlay(self.bars.clone(), feed, ART))
            .into_any_element()
    }

    /// The thumb key for a queue entry's cover. Only this source's tracks
    /// have one here.
    fn queue_art_key(&self, key: &TrackKey) -> String {
        match key.source.as_ref() == self.source() {
            true => key.path.to_string_lossy().into_owned(),
            false => String::new(),
        }
    }

    /// A queue entry's title and artist, or its path when the library
    /// doesn't know it.
    fn queue_tags(&self, key: &TrackKey, cx: &mut Context<Self>) -> (String, String) {
        match self.state.library.read(cx).meta_for_key(key) {
            Some(meta) => (meta.title, meta.artist),
            None => (key.path.to_string_lossy().into_owned(), String::new()),
        }
    }

    /// One queue entry by its tags. `now` is the Now Playing row, which
    /// wears the bars and the accent.
    fn queue_row(
        &self,
        id: &'static str,
        ix: usize,
        key: &TrackKey,
        entry: Option<u64>,
        now: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (title, artist) = self.queue_tags(key, cx);

        let path = self.queue_art_key(key);
        let art = match now {
            true => self.playing_art(&path, cx),
            false => self.art(&path, icons::MUSIC, cx),
        };

        let picked = entry.is_some() && entry == self.queue_picked;
        let row = div()
            .id((id, ix))
            .h(ROW_H)
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_SM)
            .when(picked, |row| {
                row.bg(palette::alpha(palette::accent(), 0x26))
            })
            .child(art)
            .child(Self::two_lines(title.into(), artist.into(), now, false));

        self.queue_presses(row, key, entry, cx).into_any_element()
    }

    /// A right press opens the queue menu. A row with its queue `entry` also
    /// picks on a click and plays on a double click, the Queue panel's way.
    fn queue_presses(
        &self,
        row: gpui::Stateful<Div>,
        key: &TrackKey,
        entry: Option<u64>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        row.on_mouse_down(MouseButton::Right, {
            let key = key.clone();
            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                // The row is picked like a left press would. Without an
                // entry, the menu takes whichever is audible when pressed.
                if let Some(entry) = entry {
                    window.focus(&this.focus);
                    this.pick_queued(entry, cx);
                }

                let entry = entry.or_else(|| this.state.player.read(cx).playing_entry());
                this.queue_menu = entry.map(|entry| (entry, key.clone()));
            })
        })
        .when_some(entry, |row, entry| {
            row.cursor_pointer()
                .hover(|row| row.bg(palette::bg_control_hover()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        window.focus(&this.focus);
                        match event.click_count > 1 {
                            true => this.play_queued_entry(entry, cx),
                            false => this.pick_queued(entry, cx),
                        }
                    }),
                )
        })
    }

    /// One highlight at a time: picking a queue row clears the list's.
    fn pick_queued(&mut self, entry: u64, cx: &mut Context<Self>) {
        self.selected.clear();
        self.anchor = None;
        self.cursor = None;
        self.queue_picked = Some(entry);
        cx.notify();
    }

    /// Moves the entry up before jumping, as the Queue panel does, so the
    /// ones before it stay queued. A double click means play, so a paused
    /// player starts.
    fn play_queued_entry(&mut self, entry: u64, cx: &mut Context<Self>) {
        self.queue_picked = None;

        let player = self.state.player.read(cx);
        player.play_queued(entry);
        if !player.is_playing() {
            player.toggle_pause();
        }

        cx.notify();
    }

    /// The Queue panel's menu for a Now Playing or Up Next row, with Play
    /// Similar where the track's plugin has a radio.
    fn queue_row_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let Some((entry, key)) = self.queue_menu.clone() else {
            return self.dropdown_menu(menu, window, cx);
        };

        let panel = cx.entity().downgrade();
        let play = move |_: &mut Window, cx: &mut App| {
            panel
                .update(cx, |this, cx| this.play_queued_entry(entry, cx))
                .ok();
        };

        let on_play = play.clone();
        let player = self.state.player.clone();
        let mut menu = menu
            .item(
                PopupMenuItem::new(rox_i18n::t!("library-play"))
                    .icon(Icon::default().path(icons::PLAY))
                    .on_click(move |_, window, cx| on_play(window, cx)),
            )
            .item(
                PopupMenuItem::new(rox_i18n::t!("queue-remove", count = 1u64))
                    .icon(Icon::default().path(icons::CLOSE))
                    .on_click(move |_, _, cx| {
                        player.read(cx).remove_many_from_queue(vec![entry]);
                    }),
            );

        let similar = self.similar_from_key(&key, cx);
        match self.state.library.read(cx).id_for_key(&key) {
            Some(id) => {
                menu = panel::track_actions_with(
                    menu.separator(),
                    self.state.clone(),
                    vec![id],
                    rox_i18n::t!("queue-play-now"),
                    panel::Extras {
                        after_next: similar,
                        ..panel::Extras::default()
                    },
                    window,
                    cx,
                    play,
                );
            }

            None => menu = menu.when_some(similar, |menu, item| menu.item(item)),
        }

        self.dropdown_menu(menu.separator(), window, cx)
    }

    /// Play Similar from a queued track of this panel's plugin, read back from
    /// the row the plugin gave, when the plugin has a radio.
    fn similar_from_key(&self, key: &TrackKey, cx: &mut Context<Self>) -> Option<PopupMenuItem> {
        if key.source.as_ref() != self.source() || !plugins::has_radio(self.source()) {
            return None;
        }

        let item = key.path.to_string_lossy().into_owned();
        let track = rox_library::store::open(&self.state.library.read(cx).db_path())
            .ok()
            .and_then(|conn| members::track(&conn, self.source(), &item).ok().flatten())?;

        let panel = cx.entity().downgrade();
        Some(
            PopupMenuItem::new(rox_i18n::t!("library-play-similar"))
                .icon(Icon::default().path(icons::RADIO))
                .on_click(move |_, window, cx| {
                    let (busy, title) = (track.key.clone(), track.title.clone());
                    let seed = RadioSeed::Track(track.clone());
                    let origin = window.window_handle();
                    panel
                        .update(cx, |this, cx| {
                            this.play_similar(seed, busy, title, origin, cx)
                        })
                        .ok();
                }),
        )
    }

    fn pick_view(&mut self, view: String, cx: &mut Context<Self>) {
        let mut place = self.listing.place.clone();
        place.view = Some(view);
        self.go(place, cx);
    }

    /// None at the roots, where there's nothing to climb back to.
    fn breadcrumb(&self, cx: &mut Context<Self>) -> Div {
        // Always there, so it doesn't push the list down once the roots are
        // left. At the roots it says where the list is.
        let place = &self.listing.place;

        let top = match place.library {
            true => rox_i18n::t!("source-browser-library"),
            false => rox_i18n::t!("source-browser-home"),
        };
        let mut crumbs: Vec<(Option<usize>, SharedString)> = vec![(Some(0), top)];

        crumbs.extend(place.trail.iter().enumerate().map(|(ix, crumb)| {
            let title = match &crumb.query {
                Some(query) => rox_i18n::t!("source-browser-results", query = query.clone()),
                None => SharedString::from(crumb.title.clone()),
            };
            (Some(ix + 1), title)
        }));

        if let Some(query) = &place.query {
            crumbs.push((
                None,
                rox_i18n::t!("source-browser-results", query = query.clone()),
            ));
        }

        let last = crumbs.len() - 1;
        let row = div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_XS)
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .min_w_0()
            .overflow_hidden()
            .text_xs()
            .border_b_1()
            .border_color(palette::border());

        crumbs
            .into_iter()
            .enumerate()
            .fold(row, |row, (ix, (depth, title))| {
                let crumb = div()
                    .id(("source-crumb", ix))
                    .flex_shrink()
                    .min_w_0()
                    .truncate()
                    .text_color(match ix == last {
                        true => palette::text(),
                        false => palette::text_muted(),
                    })
                    .child(title);

                let crumb = match depth {
                    Some(depth) => crumb
                        .cursor_pointer()
                        .hover(|crumb| crumb.text_color(palette::text_bright()))
                        .on_click(cx.listener(move |this, _, _, cx| this.go_up(depth, cx))),

                    None => crumb,
                };

                let row = match ix {
                    0 => row,
                    _ => row.child(
                        svg()
                            .flex_none()
                            .path(icons::CHEVRON_RIGHT)
                            .size(px(12.))
                            .text_color(palette::text_faint()),
                    ),
                };
                row.child(crumb)
            })
    }

    fn list(&mut self, cx: &mut Context<Self>) -> Div {
        let count = self.listing.entries.len();

        let list = if count == 0 {
            self.empty_state()
        } else {
            let visuals = Rc::new(layout(&self.listing.entries, self.columns()));
            let line_h = self.line_tile() + (SHELF_H - TILE);
            let sizes: Rc<Vec<gpui::Size<Pixels>>> = Rc::new(
                visuals
                    .iter()
                    .map(|visual| match visual {
                        Visual::Row(_) => size(px(1.), ROW_H),
                        Visual::Shelf(_) => size(px(1.), SHELF_H),
                        Visual::Line(_) => size(px(1.), line_h),
                    })
                    .collect(),
            );

            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .relative()
                .flex()
                .flex_col()
                .child(
                    v_virtual_list(
                        cx.entity(),
                        "source-rows",
                        sizes,
                        move |this, range, _, cx| this.rows(&visuals, range, cx),
                    )
                    .track_scroll(&self.scroll)
                    .flex_1()
                    .w_full(),
                )
                // A shelf drag's window handlers arm in a paint pass, and this
                // canvas is the paint hook, the album grid's way.
                .child(
                    canvas(|_, _, _| (), {
                        let flick = self.flick.clone();
                        let shelf = self
                            .flicking
                            .and_then(|start| self.shelves.get(&start).cloned());
                        let weak = cx.entity().downgrade();

                        move |_, _, window, _| {
                            let Some(shelf) = shelf else {
                                return;
                            };

                            panel::flick_on_paint_axis(
                                &flick,
                                Axis::Horizontal,
                                window,
                                move |d, cx| {
                                    slide(&shelf, px(d));
                                    if let Some(this) = weak.upgrade() {
                                        this.update(cx, |_, cx| cx.notify());
                                    }
                                },
                            );
                        }
                    })
                    .absolute()
                    .size_full(),
                )
        };

        // A right press clears the target on the way down; a row's handler
        // runs after and sets it back.
        list.capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
            if event.button == MouseButton::Right {
                this.menu_row = None;
            }
        }))
    }

    /// The visible rows and shelves. Nearing the end of what's listed asks
    /// for the next page, so a list shorter than the panel keeps paging until
    /// it fills.
    fn rows(
        &mut self,
        visuals: &[Visual],
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if range.end + PAGE_AHEAD >= visuals.len() {
            self.load_more(cx);
        }

        let measured = self.scroll.base_handle().bounds().size.width;
        if measured > px(0.) && measured != self.width {
            self.width = measured;
            cx.notify();
        }

        visuals[range.start.min(visuals.len())..range.end.min(visuals.len())]
            .iter()
            .filter_map(|visual| {
                let (first, element) = match visual {
                    Visual::Row(ix) => (*ix, self.row(visuals, *ix, cx)?),
                    Visual::Shelf(entries) => (entries.start, self.shelf(entries.clone(), cx)),
                    Visual::Line(entries) => (entries.start, self.line(entries.clone(), cx)),
                };

                Some(match self.fade(first) {
                    Some(opacity) => div()
                        .w_full()
                        .opacity(opacity)
                        .child(element)
                        .into_any_element(),
                    None => element,
                })
            })
            .collect()
    }

    /// How far into its fade a row that just landed is. None once it's done
    /// or for a row that was already there.
    fn fade(&self, ix: usize) -> Option<f32> {
        let (from, at) = self.fresh?;
        let t = at.elapsed().as_secs_f32() / tokens::EASE_SECS;

        // Even in and out: an ease-out lands almost at once and reads as a pop.
        (ix >= from && t < 1.).then(|| gpui::ease_in_out(t))
    }

    fn fading(&self) -> bool {
        self.fresh
            .is_some_and(|(_, at)| at.elapsed().as_secs_f32() < tokens::EASE_SECS)
    }

    fn row(&mut self, visuals: &[Visual], ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let entry = self.listing.entries.get(ix)?.clone();
        Some(match entry {
            Entry::Node {
                id,
                title,
                subtitle,
                collection,
                kind,
                art,
                ..
            } => self.node_row(ix, id, title, subtitle, collection, kind, art, cx),

            Entry::Track(track) => self.track_row(ix, &track, cx),

            Entry::Section { title, .. } => {
                self.section_row(ix, title, shelf_under(visuals, ix), cx)
            }
        })
    }

    fn row_shell(&self, ix: usize, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let selected = self.selected.contains(&ix);

        let row = div()
            .id(("source-row", ix))
            .h(ROW_H)
            .w_full()
            .min_w_0()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_SM)
            .cursor_pointer()
            .when(selected, |row| {
                row.bg(palette::alpha(palette::accent(), 0x26))
            })
            .hover(|row| row.bg(palette::bg_control_hover()));

        Self::pressable(row, ix, cx)
    }

    /// A row's or a tile's presses. The press picks, not the click, so the
    /// highlight is already right when a right press opens the menu.
    fn pressable<E: InteractiveElement>(element: E, ix: usize, cx: &mut Context<Self>) -> E {
        element
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus);
                    if event.click_count == 1 {
                        this.select(ix, event.modifiers, cx);
                    }
                }),
            )
            // The press keeps going so the list's handler sees it. A press
            // outside the set picks just that row, so the menu never acts on
            // unseen picks.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.menu_row = Some(ix);
                    if !this.selected.contains(&ix) {
                        window.focus(&this.focus);
                        this.select(ix, Modifiers::default(), cx);
                    }
                }),
            )
    }

    /// Tiles that fit across the list, which is also how far an arrow pages
    /// a shelf.
    fn columns(&self) -> usize {
        if self.width <= px(0.) {
            return FALLBACK_COLS;
        }

        let room = self.width - tokens::SPACE_SM * 2. + tokens::SPACE_MD;
        ((room / (TILE + tokens::SPACE_MD)).floor() as usize).max(1)
    }

    /// A grid line's cover edge: the columns share the whole width, so a
    /// wrapped page fills the panel instead of leaving a strip on the right.
    fn line_tile(&self) -> Pixels {
        if self.width <= px(0.) {
            return TILE;
        }

        let cols = self.columns() as f32;
        let room = self.width - tokens::SPACE_SM * 2. - tokens::SPACE_MD * (cols - 1.);
        px((f32::from(room) / cols).max(f32::from(TILE)))
    }

    /// Covers side by side, the shape a shelf and a grid line share.
    fn strip(tiles: Vec<AnyElement>, height: Pixels) -> Div {
        div()
            .h(height)
            .w_full()
            .flex()
            .flex_row()
            .gap(tokens::SPACE_MD)
            .px(tokens::SPACE_SM)
            .pt(tokens::SPACE_XS)
            .children(tiles)
    }

    /// The entries under a tiles section, as covers scrolled sideways. A
    /// vertical wheel over it still scrolls the list; shift turns it sideways,
    /// and so do the arrows on its heading.
    fn shelf(&mut self, entries: std::ops::Range<usize>, cx: &mut Context<Self>) -> AnyElement {
        let handle = self.shelves.entry(entries.start).or_default().clone();
        let tiles: Vec<AnyElement> = entries
            .clone()
            .filter_map(|ix| self.tile(ix, TILE, cx))
            .collect();

        let view = cx.entity_id();
        let sideways = handle.clone();
        let start = entries.start;
        let mut shelf = Self::strip(tiles, SHELF_H)
            .id(("source-shelf", start))
            .overflow_x_scroll()
            .track_scroll(&handle)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.flick.begin(event.position.x);
                    this.flicking = Some(start);
                    cx.notify();
                }),
            )
            .on_scroll_wheel(move |event: &gpui::ScrollWheelEvent, _, cx| {
                if !event.modifiers.shift {
                    return;
                }

                let delta = event.delta.pixel_delta(px(20.));
                slide(&sideways, if delta.x == px(0.) { delta.y } else { delta.x });

                cx.stop_propagation();
                cx.notify(view);
            });

        // Only sideways input moves a shelf, so the list keeps the wheel.
        shelf.style().restrict_scroll_to_axis = Some(true);

        shelf.into_any_element()
    }

    /// One line of a page that's a single tiles section, wrapped to the list.
    fn line(&mut self, entries: std::ops::Range<usize>, cx: &mut Context<Self>) -> AnyElement {
        let side = self.line_tile();
        let tiles: Vec<AnyElement> = entries
            .clone()
            .filter_map(|ix| self.tile(ix, side, cx))
            .collect();

        Self::strip(tiles, side + (SHELF_H - TILE))
            .id(("source-line", entries.start))
            .into_any_element()
    }

    /// A released shelf drag keeps going and slows, a frame at a time.
    fn coast(&mut self, window: &mut Window) {
        let dt = self.flick_tick.elapsed().as_secs_f32().min(0.05);
        self.flick_tick = Instant::now();

        let Some(shelf) = self.flicking.and_then(|start| self.shelves.get(&start)) else {
            return;
        };

        if let Some(d) = self.flick.coast(dt) {
            slide(shelf, px(d));
            window.request_animation_frame();
        }
    }

    /// One entry as a cover over its title. A node opens on a click, a track
    /// plays on a double click, the way their rows do.
    fn tile(&self, ix: usize, side: Pixels, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (title, second, art, glyph, node, keep) = match self.listing.entries.get(ix)? {
            Entry::Node {
                id,
                title,
                subtitle,
                art,
                kind,
                collection,
                ..
            } => (
                title.clone(),
                subtitle.clone(),
                art.clone(),
                node_glyph(*kind, *collection),
                true,
                collection.then(|| {
                    let synced = self.is_synced(id);
                    self.keep_button(ix, id, title, synced, KeepOn::Tile, cx)
                }),
            ),

            Entry::Track(track) => (
                track.title.clone(),
                track.artist.clone(),
                track.key.clone(),
                icons::MUSIC,
                false,
                None,
            ),

            Entry::Section { .. } => return None,
        };

        let selected = self.selected.contains(&ix);
        let cover = div()
            .relative()
            .child(self.art_sized(&art, glyph, side, cx))
            .when(selected, |cover| {
                cover.child(
                    div()
                        .absolute()
                        .inset_0()
                        .border_2()
                        .rounded(tokens::RADIUS)
                        .border_color(palette::accent()),
                )
            })
            .children(keep.map(|keep| {
                div()
                    .absolute()
                    .top(tokens::SPACE_XS)
                    .right(tokens::SPACE_XS)
                    .child(keep)
            }));

        let tile = div()
            .id(("source-tile", ix))
            .group(NODE_TILE)
            .flex_none()
            .w(side)
            .flex()
            .flex_col()
            .gap(px(2.))
            .cursor_pointer()
            .child(cover)
            .child(
                div()
                    .truncate()
                    .text_sm()
                    .text_color(palette::text())
                    .child(SharedString::from(title)),
            )
            .child(
                div()
                    .truncate()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(SharedString::from(second)),
            )
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                // A press that dragged its shelf was a scroll, not a click.
                if this.flicking.is_some() && this.flick.scrolled() {
                    return;
                }

                if node || event.click_count() >= 2 {
                    this.activate(ix, cx);
                }
            }));

        Some(Self::pressable(tile, ix, cx).into_any_element())
    }

    /// `busy` spins beside the title while a pick or an open runs, the
    /// library row's way.
    fn two_lines(title: SharedString, second: SharedString, playing: bool, busy: bool) -> Div {
        let title = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .min_w_0()
            .when(busy, |line| {
                line.child(Spinner::new().xsmall().color(palette::accent().into()))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .text_color(match playing {
                        true => palette::accent(),
                        false => palette::text(),
                    })
                    .child(title),
            );

        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(1.))
            .child(title)
            .when(!second.is_empty(), |lines| {
                lines.child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(second),
                )
            })
    }

    fn glyph(path: &'static str) -> AnyElement {
        svg()
            .flex_none()
            .path(path)
            .size(px(16.))
            .text_color(palette::text_faint())
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn node_row(
        &self,
        ix: usize,
        id: String,
        title: String,
        subtitle: String,
        collection: bool,
        kind: Option<NodeKind>,
        art: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let synced = self.is_synced(&id);

        // Plugin text renders as it came, never through the translator.
        let second = match (synced, self.members.get(&id)) {
            (true, Some(&count)) if !self.syncing.contains(&id) => kept_line(&subtitle, count),
            _ => SharedString::from(subtitle),
        };

        let glyph = node_glyph(kind, collection);

        let keep = collection.then(|| self.keep_button(ix, &id, &title, synced, KeepOn::Row, cx));

        self.row_shell(ix, cx)
            .group(NODE_ROW)
            .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| this.activate(ix, cx)))
            .child(self.art(&art, glyph, cx))
            .child(Self::two_lines(
                title.into(),
                second,
                false,
                self.picking.contains(&id),
            ))
            .children(keep)
            .child(Self::glyph(icons::CHEVRON_RIGHT))
            .children(self.field_cells(ix))
            .into_any_element()
    }

    /// Shows on the hovered row or tile, and stays once the collection is
    /// kept.
    fn keep_button(
        &self,
        ix: usize,
        id: &str,
        title: &str,
        synced: bool,
        on: KeepOn,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let syncing = self.syncing.contains(id);
        let kept = self.kept_look(ix).unwrap_or_else(|| SyncedCollection {
            id: id.to_string(),
            title: title.to_string(),
            ..SyncedCollection::default()
        });

        let tip = match synced {
            true => rox_i18n::t!("source-browser-synced"),
            false => rox_i18n::t!("source-browser-sync"),
        };

        let face = match syncing {
            true => Spinner::new()
                .xsmall()
                .color(palette::accent().into())
                .into_any_element(),

            false => svg()
                .path(match (on, synced) {
                    (KeepOn::Tile, false) => icons::PLUS,
                    _ => icons::CHECK,
                })
                .size(px(14.))
                .text_color(match synced {
                    true => palette::accent(),
                    false => palette::text_muted(),
                })
                .into_any_element(),
        };

        let (element, group) = match on {
            KeepOn::Row => ("source-keep", NODE_ROW),
            KeepOn::Tile => ("source-tile-keep", NODE_TILE),
        };

        div()
            .id((element, ix))
            .flex_none()
            .size(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(tokens::RADIUS)
            .cursor_pointer()
            // Over a cover the button needs an opaque floor, or the art
            // shows through the glyph. A kept one says so in its glyph.
            .map(|button| match (on, synced) {
                (KeepOn::Tile, _) => button
                    .bg(palette::bg_menu_opaque())
                    .hover(|button| button.bg(palette::bg_control_hover_opaque())),

                (KeepOn::Row, true) => button
                    .bg(palette::alpha(palette::accent(), 0x26))
                    .hover(|button| button.bg(palette::bg_control_hover())),

                (KeepOn::Row, false) => {
                    button.hover(|button| button.bg(palette::bg_control_hover()))
                }
            })
            .when(!synced && !syncing, |button| {
                button
                    .opacity(0.)
                    .group_hover(group, |button| button.opacity(1.))
            })
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            // The press stops here, or it would open the row or tile too.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                if !this.syncing.contains(&kept.id) {
                    this.set_synced(kept.clone(), !synced, cx);
                }
            }))
            .child(face)
            .into_any_element()
    }

    fn held(&self, key: &str, cx: &App) -> Held {
        let Some(id) = self.ids.get(key).copied() else {
            return Held::Out;
        };

        let library = self.state.library.read(cx);
        match (library.is_saved(id), library.in_library(id)) {
            (true, _) => Held::Saved,
            (false, true) => Held::Kept,
            (false, false) => Held::Out,
        }
    }

    /// A track's twin of the keep button: shows on the hovered row and stays
    /// lit while the track is in the library. A kept collection's track is
    /// lit but doesn't toggle, since taking it out is the collection's call.
    fn track_check(&self, ix: usize, track: &PluginTrack, cx: &mut Context<Self>) -> AnyElement {
        let held = self.held(&track.key, cx);

        let tip = match held {
            Held::Out => rox_i18n::t!("source-browser-track-save"),
            Held::Saved => rox_i18n::t!("source-browser-track-saved"),
            Held::Kept => rox_i18n::t!("source-browser-track-kept"),
        };

        let face = svg()
            .path(icons::CHECK)
            .size(px(14.))
            .text_color(match held {
                Held::Out => palette::text_muted(),
                Held::Saved | Held::Kept => palette::accent(),
            });

        let button = div()
            .id(("source-track-check", ix))
            .flex_none()
            .size(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(tokens::RADIUS)
            .tooltip(move |window, cx| {
                gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
            })
            // The press stops here, or it would select the row too.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(face);

        let panel = cx.entity().downgrade();
        let (picked, key) = (track.clone(), track.key.clone());

        match held {
            Held::Out => button
                .cursor_pointer()
                .opacity(0.)
                .group_hover(TRACK_ROW, |button| button.opacity(1.))
                .hover(|button| button.bg(palette::bg_control_hover()))
                .on_click(move |_, _, cx| {
                    let tracks = vec![picked.clone()];
                    panel.update(cx, |this, cx| this.save(tracks, cx)).ok();
                }),

            Held::Saved => button
                .cursor_pointer()
                .bg(palette::alpha(palette::accent(), 0x26))
                .hover(|button| button.bg(palette::bg_control_hover()))
                .on_click(move |_, _, cx| {
                    let paths = vec![key.clone()];
                    panel.update(cx, |this, cx| this.unsave(paths, cx)).ok();
                }),

            Held::Kept => button.bg(palette::alpha(palette::accent(), 0x26)),
        }
        .into_any_element()
    }

    /// A heading the plugin put over the rows after it. Nothing to pick.
    /// Over a shelf wider than the list, it carries the arrows that page it:
    /// the only sideways control a plain mouse wheel has.
    fn section_row(
        &mut self,
        ix: usize,
        title: String,
        shelf: Option<std::ops::Range<usize>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cols = self.columns();
        let arrows = shelf.filter(|run| run.len() > cols).map(|run| {
            let handle = self.shelves.entry(run.start).or_default().clone();
            let page = (TILE + tokens::SPACE_MD) * cols as f32;
            let view = cx.entity_id();

            let arrow = move |icon: &'static str, id: &'static str, by: Pixels| {
                let handle = handle.clone();
                icon_button(icon, false, move |_, _, cx| {
                    slide(&handle, by);
                    cx.notify(view);
                })
                .keyed((id, ix))
            };

            div()
                .flex()
                .flex_row()
                .gap(px(2.))
                .child(arrow(icons::CHEVRON_LEFT, "source-shelf-back", page))
                .child(arrow(icons::CHEVRON_RIGHT, "source-shelf-on", -page))
        });

        div()
            .id(("source-section", ix))
            .h(ROW_H)
            .w_full()
            .flex()
            .flex_row()
            .items_end()
            .justify_between()
            .px(tokens::SPACE_SM)
            .pb(tokens::SPACE_XS)
            .text_xs()
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(palette::text_muted())
            .child(SharedString::from(title))
            .children(arrows)
            .into_any_element()
    }

    fn track_row(&self, ix: usize, track: &PluginTrack, cx: &mut Context<Self>) -> AnyElement {
        let key = self.key_for(&track.key);
        let playing = self.playing.as_ref() == Some(&key);
        let busy = self.picking.contains(&track.key) || self.opening.as_ref() == Some(&key);

        let duration = match track.duration_ms {
            0 => SharedString::default(),
            ms => fmt_time(f64::from(ms) / 1000.0).into(),
        };

        let album = self
            .listing
            .place
            .node()
            .is_some_and(|crumb| crumb.kind == Some(NodeKind::Album));
        let number = album.then(|| {
            div()
                .flex_none()
                .w(TRACK_NO_W)
                .text_right()
                .text_xs()
                .text_color(palette::text_muted())
                .child(fmt_num(track.track_no))
        });

        let check = self.track_check(ix, track, cx);

        self.row_shell(ix, cx)
            .group(TRACK_ROW)
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                if event.click_count() >= 2 {
                    this.activate(ix, cx);
                }
            }))
            .children(number)
            .child(match playing {
                true => self.playing_art(&track.key, cx),
                false => self.art(&track.key, icons::MUSIC, cx),
            })
            .child(Self::two_lines(
                track.title.clone().into(),
                track.artist.clone().into(),
                playing,
                busy,
            ))
            .child(check)
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(duration),
            )
            .children(self.field_cells(ix))
            .into_any_element()
    }

    /// Fetched through the plugin until it's stored, so a track that isn't a
    /// library row yet still shows its cover. An empty key, a node with no
    /// art, keeps the slot so rows line up.
    fn art(&self, key: &str, placeholder: &'static str, cx: &mut Context<Self>) -> AnyElement {
        self.art_sized(key, placeholder, ART, cx)
    }

    fn art_sized(
        &self,
        key: &str,
        placeholder: &'static str,
        side: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let source = self.config.source.clone();
        let thumb = match key.is_empty() {
            true => Thumb::Missing,
            false => self
                .state
                .thumbs
                .update(cx, |thumbs, cx| thumbs.get_plugin(&source, key, cx)),
        };

        match thumb {
            // gpui paints a covering image past its own bounds, so the box
            // around it does the clipping. Art stored before plugin covers
            // were cropped square can still be any shape.
            Thumb::Ready(image) => div()
                .flex_none()
                .size(side)
                .overflow_hidden()
                .rounded(tokens::RADIUS)
                .child(
                    img(image)
                        .size_full()
                        .object_fit(ObjectFit::Cover)
                        .rounded(tokens::RADIUS),
                )
                .into_any_element(),

            _ => div()
                .flex_none()
                .size(side)
                .flex()
                .items_center()
                .justify_center()
                .rounded(tokens::RADIUS)
                .bg(palette::bg_control())
                .child(Self::glyph(placeholder))
                .into_any_element(),
        }
    }

    fn unavailable_banner(&self, reason: Unavailable) -> Div {
        let source = self.label.to_string();
        let headline = match reason {
            Unavailable::PluginsOff => rox_i18n::t!("source-browser-plugins-off"),
            Unavailable::SwitchedOff => {
                rox_i18n::t!("source-browser-switched-off", source = source)
            }
            Unavailable::Changed => rox_i18n::t!("source-browser-changed", source = source),
            Unavailable::Missing => rox_i18n::t!("source-browser-missing", source = source),
            Unavailable::Failed => rox_i18n::t!("source-browser-cant-load", source = source),
            Unavailable::Stopped => rox_i18n::t!("source-browser-stopped", source = source),
        };

        panel::banner(Tone::Warn, headline, Vec::new()).child(open_plugins_button())
    }

    /// Blank while a notice already says why the place is empty.
    fn empty_state(&self) -> Div {
        let text = match self.listing.loading() || self.listing.notice.is_some() {
            true => SharedString::default(),
            false => rox_i18n::t!("source-browser-empty"),
        };

        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .p(tokens::SPACE_MD)
            .text_sm()
            .text_color(palette::text_muted())
            .child(text)
    }

    /// Read when drawn: the running sources are a lock and a small map.
    fn picker(&self, cx: &mut Context<Self>) -> Div {
        let sources = plugins::live_sources();

        let body = div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(tokens::SPACE_SM)
            .p(tokens::SPACE_MD)
            .text_center()
            .child(div().text_lg().child(rox_i18n::t!("source-browser-pick")));

        if sources.is_empty() {
            return body.child(
                div()
                    .text_sm()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("source-browser-no-sources")),
            );
        }

        body.children(sources.into_iter().map(|(source, label)| {
            let chosen = source.clone();
            crate::settings::ui::small_button(
                SharedString::from(label),
                icons::GLOBE,
                false,
                cx.listener(move |this, _, _, cx| this.choose(chosen.clone(), cx)),
            )
        }))
    }

    fn row_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let Some(row) = self.menu_row else {
            return self.dropdown_menu(menu, window, cx);
        };

        let rows = match self.selected.contains(&row) {
            true => self.selected_rows(),
            false => vec![row],
        };
        let similar = (rows.len() == 1)
            .then(|| self.similar_item(row, cx))
            .flatten();
        let tracks = self.tracks_at(&rows);
        if tracks.is_empty() {
            let menu = self.node_items(menu, &rows, similar, cx).separator();
            let menu = match (rows.as_slice(), self.listing.entries.get(row)) {
                ([_], Some(Entry::Node { id, .. })) => {
                    panel::link_items(menu, self.source(), id.clone())
                }
                _ => menu,
            };

            let nodes: Vec<String> = rows
                .iter()
                .filter_map(|row| match self.listing.entries.get(*row) {
                    Some(Entry::Node { id, .. }) => Some(id.clone()),
                    _ => None,
                })
                .collect();
            let menu =
                plugin_actions::offer(menu, self.source(), "node", nodes, &self.listing.flags);

            return self.dropdown_menu(menu.separator(), window, cx);
        }

        let play_label = match tracks.len() {
            1 => rox_i18n::t!("source-browser-play"),
            n => rox_i18n::t!("source-browser-play-count", count = n as u64),
        };

        // Rows already in the library get the whole shared menu. Anything
        // else gets the actions a pick can serve first.
        let ids: Option<Vec<i64>> = tracks
            .iter()
            .map(|track| self.ids.get(&track.key).copied())
            .collect();

        // One row plays on through the list like a double click; a set plays
        // as a run of its own.
        let single = (rows.len() == 1).then_some(row);
        let (panel, play_tracks) = (cx.entity().downgrade(), tracks.clone());
        let on_play = move |_: &mut Window, cx: &mut App| {
            let tracks = play_tracks.clone();
            panel
                .update(cx, |this, cx| match single {
                    Some(ix) => this.play_from(ix, cx),
                    None => this.play(tracks, 0, this.listing.place.clone(), cx),
                })
                .ok();
        };

        cx.set_global(MenuOrigin(cx.entity().downgrade()));

        // The listing's Go to is the plugin's newest and skips the node shown,
        // so it stands in for the one the library kept.
        let go_to = match tracks.as_slice() {
            [track] => self.go_to_submenu(&track.key, window, cx),
            _ => None,
        };
        let add = self.library_item(&tracks, cx);

        let menu = match ids {
            Some(ids) => {
                let extras = panel::Extras {
                    after_next: similar,
                    go_to,
                    plugin: add.into_iter().collect(),
                };
                panel::track_actions_with(
                    menu,
                    self.state.clone(),
                    ids,
                    play_label,
                    extras,
                    window,
                    cx,
                    on_play,
                )
            }

            // A track not in the library yet gets the shared menu's plugin
            // section here, in the same order.
            None => {
                let menu = self
                    .unpicked_menu(
                        menu,
                        tracks.clone(),
                        play_label,
                        on_play,
                        similar,
                        window,
                        cx,
                    )
                    .separator();
                let menu = match tracks.as_slice() {
                    [track] => panel::link_items(menu, self.source(), track.key.clone()),
                    _ => menu,
                };

                let keys = tracks.iter().map(|track| track.key.clone()).collect();
                let menu =
                    plugin_actions::offer(menu, self.source(), "track", keys, &self.listing.flags);

                menu.when_some(add, |menu, item| menu.item(item))
                    .separator()
                    .when_some(go_to, |menu, item| menu.item(item))
            }
        };

        self.dropdown_menu(menu.separator(), window, cx)
    }

    /// Rows already in the library keep the Go to this listing showed, so a
    /// row from before Go to was kept gets it everywhere it's listed.
    fn keep_listed_go_to(&self, cx: &mut Context<Self>) {
        let listed: HashMap<String, GoTo> = self
            .listing
            .go_to
            .iter()
            .filter(|(key, _)| self.ids.contains_key(*key))
            .map(|(key, go_to)| (key.clone(), go_to.clone()))
            .collect();
        if listed.is_empty() {
            return;
        }

        let (db, source) = (
            self.state.library.read(cx).db_path(),
            self.source().to_string(),
        );
        cx.background_executor()
            .spawn(async move { plugins::keep_go_to(&db, &source, &listed) })
            .detach();
    }

    /// Go to the track's album or one of its artists, when the plugin named
    /// them. The node the list is already showing isn't offered.
    fn go_to_submenu(
        &self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PopupMenuItem> {
        let go_to = self.listing.go_to.get(key)?;

        let here = self.listing.place.node().map(|crumb| crumb.id.as_str());
        let elsewhere = |target: &&Target| Some(target.id.as_str()) != here;
        let album = go_to.album.iter().find(elsewhere).cloned();
        let artists: Vec<Target> = go_to.artists.iter().filter(elsewhere).cloned().collect();

        if album.is_none() && artists.is_empty() {
            return None;
        }

        let panel = cx.entity().downgrade();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, window, cx| {
            if let Some(album) = album {
                let label = rox_i18n::t!("source-browser-go-to-album");
                submenu = submenu.item(go_to_item(label, album, panel.clone()));
            }

            let label = rox_i18n::t!("source-browser-go-to-artist");
            match artists.as_slice() {
                [] => {}

                [artist] => {
                    submenu = submenu.item(go_to_item(label, artist.clone(), panel.clone()));
                }

                // Several artists get one entry each, by name.
                several => {
                    let several = several.to_vec();
                    let names = PopupMenu::build(window, cx, move |mut names, _, _| {
                        for artist in several {
                            let name = SharedString::from(artist.title.clone());
                            names = names.item(go_to_item(name, artist, panel.clone()));
                        }
                        names
                    });
                    submenu = submenu.item(
                        PopupMenuItem::submenu(label, names)
                            .icon(Icon::default().path(icons::USER)),
                    );
                }
            }

            submenu
        });

        Some(
            PopupMenuItem::submenu(rox_i18n::t!("source-browser-go-to"), submenu)
                .icon(Icon::default().path(icons::ARROW_RIGHT)),
        )
    }

    /// Play Similar, when the plugin has a radio, seeded from the row.
    fn similar_item(&self, row: usize, cx: &mut Context<Self>) -> Option<PopupMenuItem> {
        if !plugins::has_radio(self.source()) {
            return None;
        }

        let (busy, title) = match self.listing.entries.get(row)? {
            Entry::Track(track) => (track.key.clone(), track.title.clone()),
            Entry::Node { id, title, .. } => (id.clone(), title.clone()),
            Entry::Section { .. } => return None,
        };

        let panel = cx.entity().downgrade();
        Some(
            PopupMenuItem::new(rox_i18n::t!("library-play-similar"))
                .icon(Icon::default().path(icons::RADIO))
                .on_click(move |_, window, cx| {
                    let (busy, title) = (busy.clone(), title.clone());
                    let origin = window.window_handle();
                    panel
                        .update(cx, |this, cx| {
                            let seed = match this.listing.entries.get(row) {
                                Some(Entry::Track(track)) => RadioSeed::Track(track.clone()),
                                _ => RadioSeed::Node(busy.clone()),
                            };
                            this.play_similar(seed, busy, title, origin, cx)
                        })
                        .ok();
                }),
        )
    }

    /// Play, Play Next and Add to Queue for nodes: everything they list,
    /// read from the plugin first.
    fn node_items(
        &self,
        menu: PopupMenu,
        rows: &[usize],
        similar: Option<PopupMenuItem>,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let nodes: Vec<String> = rows
            .iter()
            .filter_map(|&ix| match self.listing.entries.get(ix) {
                Some(Entry::Node { id, .. }) => Some(id.clone()),
                _ => None,
            })
            .collect();
        if nodes.len() != 1 {
            return menu;
        }

        let node = nodes[0].clone();
        let panel = cx.entity().downgrade();
        let item = |label: SharedString, icon: &'static str, how: NodePlay| {
            let (panel, node) = (panel.clone(), node.clone());
            PopupMenuItem::new(label)
                .icon(Icon::default().path(icon))
                .on_click(move |_, _, cx| {
                    let node = node.clone();
                    panel
                        .update(cx, |this, cx| this.play_node(node, how, cx))
                        .ok();
                })
        };

        menu.item(item(
            rox_i18n::t!("source-browser-play"),
            icons::PLAY,
            NodePlay::Play,
        ))
        .item(item(
            rox_i18n::t!("panel-play-next"),
            icons::SKIP_FORWARD,
            NodePlay::Next,
        ))
        .when_some(similar, |menu, item| menu.item(item))
        .item(item(
            rox_i18n::t!("panel-add-to-queue"),
            icons::LIST_MUSIC,
            NodePlay::Queue,
        ))
    }

    /// Playing a track doesn't add it (ADR 29), so adding is its own item.
    /// Taking rows back out is the shared menu's, since only rows already in
    /// the library can be.
    fn library_item(
        &self,
        tracks: &[PluginTrack],
        cx: &mut Context<Self>,
    ) -> Option<PopupMenuItem> {
        let library = self.state.library.read(cx);
        let missing: Vec<PluginTrack> = tracks
            .iter()
            .filter(|track| {
                !self
                    .ids
                    .get(&track.key)
                    .is_some_and(|id| library.in_library(*id))
            })
            .cloned()
            .collect();
        if missing.is_empty() {
            return None;
        }

        let panel = cx.entity().downgrade();
        Some(
            PopupMenuItem::new(rox_i18n::t!("source-browser-add-to-library"))
                .icon(Icon::default().path(icons::PLUS))
                .on_click(move |_, _, cx| {
                    let tracks = missing.clone();
                    panel.update(cx, |this, cx| this.save(tracks, cx)).ok();
                }),
        )
    }

    fn save(&mut self, tracks: Vec<PluginTrack>, cx: &mut Context<Self>) {
        self.failure = None;
        let task = plugins::save(self.state.library.clone(), self.source(), tracks, cx);
        self.after_write(task, "source-browser-save-failed", cx);
    }

    fn unsave(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        self.failure = None;
        let task = plugins::unsave(self.state.library.clone(), self.source(), paths, cx);
        self.after_write(task, "source-browser-unsave-failed", cx);
    }

    /// The rows may have come or gone, so the ids are read again.
    fn after_write<T: 'static>(
        &mut self,
        task: Task<Result<T, String>>,
        failed: &'static str,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(_) => this.resolve_ids(cx),
                    Err(e) => this.failure = Some((rox_i18n::t!(failed), e)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn unpicked_menu(
        &self,
        menu: PopupMenu,
        tracks: Vec<PluginTrack>,
        play_label: SharedString,
        on_play: impl Fn(&mut Window, &mut App) + 'static,
        similar: Option<PopupMenuItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let panel = cx.entity().downgrade();

        let item = |label: SharedString,
                    icon: &'static str,
                    act: fn(&mut Self, Vec<PluginTrack>, &mut Context<Self>)| {
            let (panel, tracks) = (panel.clone(), tracks.clone());
            PopupMenuItem::new(label)
                .icon(Icon::default().path(icon))
                .on_click(move |_, _, cx| {
                    let tracks = tracks.clone();
                    panel.update(cx, |this, cx| act(this, tracks, cx)).ok();
                })
        };

        let menu = menu
            .item(
                PopupMenuItem::new(play_label)
                    .icon(Icon::default().path(icons::PLAY))
                    .on_click(move |_, window, cx| on_play(window, cx)),
            )
            .item(item(
                rox_i18n::t!("panel-play-next"),
                icons::SKIP_FORWARD,
                |this, tracks, cx| this.queue(tracks, true, cx),
            ))
            .when_some(similar, |menu, item| menu.item(item))
            .item(item(
                rox_i18n::t!("panel-add-to-queue"),
                icons::LIST_MUSIC,
                |this, tracks, cx| this.queue(tracks, false, cx),
            ));

        let with_ids: WithIds = Rc::new(move |then: Then, cx: &mut App| {
            let tracks = tracks.clone();
            panel
                .update(cx, |this, cx| {
                    let library = this.state.library.clone();
                    this.with_picked(
                        tracks,
                        None,
                        move |keys, cx| {
                            let ids = {
                                let library = library.read(cx);
                                keys.iter()
                                    .filter_map(|key| library.id_for_key(key))
                                    .collect()
                            };
                            then(ids, cx);
                        },
                        cx,
                    );
                })
                .ok();
        });

        panel::playlist_item_deferred(menu, self.state.clone(), with_ids, window, cx)
    }
}

impl PanelSettings for SourceBrowserPanel {
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
        &[("Content", icons::LAYOUT_DASHBOARD)]
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
            .child(panel::setting_row(
                rox_i18n::t!("source-browser-show-home"),
                Some(rox_i18n::t!("source-browser-show-home.description")),
                panel::toggle(
                    self.config.home,
                    |this: &mut Self, on, cx| {
                        this.config.home = on;

                        // The roots list again to take the home in or out.
                        if this.listing.place.is_root() {
                            this.go(Place::default(), cx);
                        }
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .into_any_element()
    }
}

impl EventEmitter<PanelEvent> for SourceBrowserPanel {}

impl Focusable for SourceBrowserPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for SourceBrowserPanel {
    fn panel_name(&self) -> &'static str {
        "source browser"
    }

    rox_panel_api::opens_settings!();

    /// The source's own label once one is pinned.
    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let default = match self.label.is_empty() {
            true => rox_i18n::t!("source-browser-title"),
            false => self.label.clone(),
        };

        panel::title_text(self.config.chrome.title.as_deref(), default)
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }

    fn content_context_menu(&self, _cx: &App) -> bool {
        true
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu = match self.playing_row() {
            Some(_) => {
                let panel = cx.entity().downgrade();
                menu.item(
                    PopupMenuItem::new(rox_i18n::t!("source-browser-jump-to-playing"))
                        .icon(Icon::default().path(icons::LOCATE))
                        .on_click(move |_, _, cx| {
                            panel.update(cx, |this, cx| this.jump_to_playing(cx)).ok();
                        }),
                )
                .separator()
            }

            None => menu,
        };

        // An action on no item is the panel's, so it's in the panel's own menu.
        let menu = match plugin_actions::offers(self.source(), "source") {
            true => {
                let menu = menu.separator();
                plugin_actions::offer(menu, self.source(), "source", Vec::new(), &HashMap::new())
                    .separator()
            }
            false => menu,
        };

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
                SourceBrowserPanel::new(state, config, window, cx)
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

impl gpui::Render for SourceBrowserPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.coast(window);

        // Nothing else asks for frames while rows fade in.
        if self.fading() {
            window.request_animation_frame();
        }

        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

const PLUGINS_PAGE: &str = "settings-page-plugins";

fn value_of<'a>(row: &'a Values, id: &str) -> Option<&'a FieldValue> {
    row.iter()
        .find(|(field, _)| field == id)
        .map(|(_, value)| value)
}

/// Biggest, newest or first alphabetically on top, and a missing value last.
fn field_order(
    kind: FieldKind,
    a: Option<&FieldValue>,
    b: Option<&FieldValue>,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,

        (Some(FieldValue::Number(a)), Some(FieldValue::Number(b))) => b.total_cmp(a),

        (Some(FieldValue::Text(a)), Some(FieldValue::Text(b))) => match kind {
            FieldKind::Date => b.cmp(a),
            _ => a.to_lowercase().cmp(&b.to_lowercase()),
        },

        // A number beside text in one column: numbers first.
        (Some(FieldValue::Number(_)), Some(FieldValue::Text(_))) => Ordering::Less,
        (Some(FieldValue::Text(_)), Some(FieldValue::Number(_))) => Ordering::Greater,
    }
}

fn fmt_field(kind: FieldKind, value: &FieldValue) -> String {
    match (kind, value) {
        (FieldKind::Percent, FieldValue::Number(n)) => format!("{}%", n.round() as i64),
        (FieldKind::Count, FieldValue::Number(n)) => fmt_count(*n),
        (_, FieldValue::Number(n)) => format!("{n}"),
        (_, FieldValue::Text(text)) => text.clone(),
    }
}

/// 999, then 1.2k, 12k, 1.2M: a column fits four or five characters.
fn fmt_count(n: f64) -> String {
    let short = |v: f64, suffix: &str| {
        let text = match v >= 100.0 {
            true => format!("{v:.0}"),
            false => format!("{v:.1}"),
        };
        format!("{}{suffix}", text.trim_end_matches(".0"))
    };

    match n.abs() {
        v if v < 1e3 => format!("{}", n.round() as i64),
        v if v < 1e6 => short(n / 1e3, "k"),
        v if v < 1e9 => short(n / 1e6, "M"),
        _ => short(n / 1e9, "B"),
    }
}

fn open_plugins_button() -> impl IntoElement {
    crate::settings::ui::small_button(
        rox_i18n::t!("source-browser-open-plugins"),
        icons::PLUG,
        false,
        |_, window, cx| panel_settings::open_app_page(PLUGINS_PAGE, window, cx),
    )
}

/// Opens in the browser. The label is the plugin's own, like the notice's text.
fn open_link_button(link: NoticeLink) -> impl IntoElement {
    let label: SharedString = match link.label.trim().is_empty() {
        true => rox_i18n::t!("source-browser-open-link"),
        false => link.label.into(),
    };

    crate::settings::ui::small_button(label, icons::EXTERNAL_LINK, false, move |_, _, cx| {
        cx.open_url(&link.url)
    })
}

/// A setup notice offers the settings it's asking for, and a linked one the
/// page it names.
fn notice_banner(notice: &Notice) -> Div {
    let tone = match notice.setup {
        true => Tone::Warn,
        false => Tone::Info,
    };

    panel::banner(tone, notice.text.clone(), Vec::new())
        .when(notice.setup, |banner| banner.child(open_plugins_button()))
        .when_some(notice.link.clone(), |banner, link| {
            banner.child(open_link_button(link))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The owner rides in the flattened chrome, so a plugin panel's dump
    /// keeps naming its plugin through a save and a restore.
    #[test]
    fn a_plugin_panel_keeps_its_owner_through_a_dump() {
        let info = rox_dock::PanelInfo::Panel(serde_json::json!({
            "source": "plugin:example-tones",
            "owner": "plugin:example-tones",
        }));
        let config: SourceBrowserConfig = rox_panel_api::panel::config_from_info(&info);
        assert_eq!(config.chrome.owner, "plugin:example-tones");

        let dumped = serde_json::to_value(&config).unwrap();
        assert_eq!(dumped["owner"], "plugin:example-tones");

        let plain = serde_json::to_value(SourceBrowserConfig::default()).unwrap();
        assert!(plain.get("owner").is_none(), "a core panel writes no owner");
    }

    fn node(id: &str) -> Entry {
        Entry::Node {
            id: id.to_string(),
            title: id.to_string(),
            subtitle: String::new(),
            collection: false,
            kind: None,
            art: String::new(),
            home: false,
        }
    }

    fn page(ids: &[&str], cursor: Option<&str>) -> Page {
        Page {
            entries: ids.iter().map(|id| node(id)).collect(),
            cursor: cursor.map(str::to_string),
            ..Page::default()
        }
    }

    fn at(id: &str) -> Place {
        Place {
            trail: vec![Crumb {
                id: id.to_string(),
                title: id.to_string(),
                query: None,
                collection: true,
                kind: None,
            }],
            ..Place::default()
        }
    }

    fn ids(listing: &Listing) -> Vec<String> {
        listing
            .entries
            .iter()
            .map(|entry| match entry {
                Entry::Node { id, .. } => id.clone(),
                Entry::Track(track) => track.key.clone(),
                Entry::Section { title, .. } => title.clone(),
            })
            .collect()
    }

    #[test]
    fn a_first_page_brings_its_views_and_a_next_page_keeps_them() {
        let mut listing = Listing::default();
        let views = vec![
            View {
                id: "all".into(),
                label: "All".into(),
            },
            View {
                id: "albums".into(),
                label: "Albums".into(),
            },
        ];

        let mut albums = at("search");
        albums.view = Some("albums".into());
        let first = listing.begin(Some(albums.clone()));
        listing.land(
            first,
            Ok(Page {
                views: views.clone(),
                view: Some("albums".into()),
                ..page(&["a"], Some("1"))
            }),
        );
        assert_eq!(listing.views, views);
        assert_eq!(listing.view.as_deref(), Some("albums"));

        let next = listing.begin(None);
        listing.land(next, Ok(page(&["b"], None)));
        assert_eq!(listing.views, views, "a next page doesn't clear the views");
        assert_eq!(listing.place, albums);

        let all = listing.begin(Some(at("search")));
        assert_eq!(
            listing.land(all, Ok(page(&["x"], None))),
            Landed::Replaced { moved: true },
            "another view of the same place is a move"
        );
    }

    #[test]
    fn a_sort_keeps_values_with_their_rows_and_rows_under_their_heading() {
        let mut listing = Listing::default();
        let asked = listing.begin(Some(at("search")));
        let pop = |n: f64| vec![("pop".to_string(), FieldValue::Number(n))];

        listing.land(
            asked,
            Ok(Page {
                entries: vec![
                    Entry::Section {
                        title: "Albums".into(),
                        tiles: false,
                    },
                    node("a"),
                    node("b"),
                    node("c"),
                    Entry::Section {
                        title: "Tracks".into(),
                        tiles: false,
                    },
                    node("x"),
                    node("y"),
                ],
                fields: vec![Field {
                    id: "pop".into(),
                    label: "Popularity".into(),
                    kind: FieldKind::Percent,
                }],
                values: vec![
                    vec![],
                    pop(10.),
                    vec![],
                    pop(90.),
                    vec![],
                    pop(5.),
                    pop(50.),
                ],
                ..Page::default()
            }),
        );

        listing.sort = Some("pop".into());
        listing.apply_sort();

        assert_eq!(ids(&listing), ["Albums", "c", "a", "b", "Tracks", "y", "x"]);
        assert_eq!(listing.values[1], pop(90.), "a value moves with its row");
        assert!(listing.values[3].is_empty(), "a row without one goes last");
    }

    #[test]
    fn counts_read_short_and_percents_whole() {
        assert_eq!(
            fmt_field(FieldKind::Count, &FieldValue::Number(999.)),
            "999"
        );
        assert_eq!(
            fmt_field(FieldKind::Count, &FieldValue::Number(1234.)),
            "1.2k"
        );
        assert_eq!(
            fmt_field(FieldKind::Count, &FieldValue::Number(12_000.)),
            "12k"
        );
        assert_eq!(
            fmt_field(FieldKind::Count, &FieldValue::Number(3_400_000.)),
            "3.4M"
        );
        assert_eq!(
            fmt_field(FieldKind::Percent, &FieldValue::Number(77.6)),
            "78%"
        );
        assert_eq!(
            fmt_field(FieldKind::Date, &FieldValue::Text("2019-03".into())),
            "2019-03"
        );
    }

    #[test]
    fn a_second_page_appends() {
        let mut listing = Listing::default();

        let first = listing.begin(Some(at("liked")));
        assert_eq!(
            listing.land(first, Ok(page(&["a", "b"], Some("2")))),
            Landed::Replaced { moved: true }
        );
        assert!(listing.wants_more());

        let second = listing.begin(None);
        assert!(!listing.wants_more(), "one page in flight at a time");
        assert_eq!(
            listing.land(second, Ok(page(&["c"], None))),
            Landed::Appended
        );

        assert_eq!(ids(&listing), ["a", "b", "c"]);
        assert!(!listing.wants_more(), "a null cursor is the last page");
    }

    #[test]
    fn the_home_follows_the_roots_and_pages_on() {
        let mut listing = Listing::default();

        let mut roots = page(&["a", "home"], None);
        if let Entry::Node { home, .. } = &mut roots.entries[1] {
            *home = true;
        }

        let first = listing.begin(Some(Place::default()));
        listing.land(first, Ok(roots));
        listing.follow_home();

        assert_eq!(ids(&listing), ["a"], "the home isn't a node of its own");
        assert!(
            listing.wants_more(),
            "the roots ran out but the home hasn't"
        );
        assert_eq!(listing.next_page(), (Some("home".into()), None, true));

        let (_, _, home_first) = listing.next_page();
        let second = listing.begin_page(None, home_first);
        listing.land(second, Ok(page(&["x"], Some("2"))));
        assert_eq!(
            listing.next_page(),
            (Some("home".into()), Some("2".into()), false)
        );

        let third = listing.begin(None);
        listing.land(third, Ok(page(&["y"], None)));
        assert_eq!(ids(&listing), ["a", "x", "y"]);
        assert!(!listing.wants_more());

        // Listing the roots again starts over, home and all.
        let again = listing.begin(Some(Place::default()));
        listing.land(again, Ok(page(&["a"], None)));
        assert_eq!(listing.home, Home::None);
    }

    #[test]
    fn a_search_resets_the_list() {
        let mut listing = Listing::default();
        let first = listing.begin(Some(at("liked")));
        listing.land(first, Ok(page(&["a", "b"], Some("2"))));

        let search = Place {
            query: Some("tones".into()),
            ..Place::default()
        };
        let asked = listing.begin(Some(search.clone()));
        assert_eq!(
            listing.land(asked, Ok(page(&["x"], None))),
            Landed::Replaced { moved: true }
        );

        assert_eq!(ids(&listing), ["x"]);
        assert_eq!(listing.place, search);
        assert_eq!(listing.cursor, None);
    }

    #[test]
    fn a_failed_page_keeps_the_list() {
        let mut listing = Listing::default();
        let first = listing.begin(Some(at("liked")));
        listing.land(first, Ok(page(&["a", "b"], Some("2"))));

        let second = listing.begin(None);
        assert_eq!(
            listing.land(second, Err("the plugin timed out".into())),
            Landed::Failed
        );

        assert_eq!(ids(&listing), ["a", "b"]);
        assert_eq!(listing.error.as_deref(), Some("the plugin timed out"));
        assert_eq!(listing.cursor.as_deref(), Some("2"), "kept for a retry");
        assert!(!listing.wants_more(), "a failure doesn't retry on its own");

        // Opening somewhere else fails too, and the list still stands.
        let away = listing.begin(Some(at("mixes")));
        listing.land(away, Err("no plugin host".into()));
        assert_eq!(ids(&listing), ["a", "b"]);
        assert_eq!(listing.place, at("liked"));
    }

    #[test]
    fn an_answer_for_a_replaced_request_is_dropped() {
        let mut listing = Listing::default();
        let slow = listing.begin(Some(at("liked")));
        let fast = listing.begin(Some(at("mixes")));

        assert_eq!(
            listing.land(fast, Ok(page(&["m"], None))),
            Landed::Replaced { moved: true }
        );
        assert_eq!(listing.land(slow, Ok(page(&["l"], None))), Landed::Stale);
        assert_eq!(ids(&listing), ["m"]);
    }

    #[test]
    fn go_to_steps_back_or_sideways_instead_of_stacking() {
        let target = |id: &str, kind| Target {
            id: id.to_string(),
            title: id.to_string(),
            collection: kind == NodeKind::Album,
            kind: Some(kind),
        };
        let trail = |place: &Place| -> Vec<String> {
            place
                .trail
                .iter()
                .map(|crumb| crumb.title.clone())
                .collect()
        };

        // From a playlist the way back stays.
        let album = at("liked").going_to(&target("showgirl", NodeKind::Album));
        assert_eq!(trail(&album), ["liked", "showgirl"]);

        let artist = album.going_to(&target("taylor", NodeKind::Artist));
        assert_eq!(trail(&artist), ["liked", "taylor"], "an album gives way");

        let opened = artist.clone().opening(Crumb {
            id: "midnights".into(),
            title: "midnights".into(),
            query: None,
            collection: true,
            kind: Some(NodeKind::Album),
        });
        let back = opened.going_to(&target("taylor", NodeKind::Artist));
        assert_eq!(back, artist, "a node on the trail is stepped back to");

        let searched = Place {
            query: Some("swift".into()),
            ..Place::default()
        };
        let from_results = searched.going_to(&target("taylor", NodeKind::Artist));
        assert_eq!(trail(&from_results), ["swift", "taylor"]);
    }

    #[test]
    fn rereading_a_place_is_not_a_move() {
        let mut listing = Listing::default();
        let first = listing.begin(Some(at("liked")));
        listing.land(first, Ok(page(&["a"], None)));

        let again = listing.begin(Some(at("liked")));
        assert_eq!(
            listing.land(again, Ok(page(&["a", "b"], None))),
            Landed::Replaced { moved: false }
        );
    }

    #[test]
    fn a_play_runs_on_from_the_clicked_track() {
        assert_eq!(
            play_window(20, 3, 1000),
            (0..20, 3),
            "a short list plays whole"
        );

        // Capped: half behind for Prev, the rest ahead.
        assert_eq!(play_window(100, 50, 10), (45..55, 5));

        // Near an end, the short side's share goes to the other.
        assert_eq!(play_window(100, 2, 10), (0..10, 2));
        assert_eq!(play_window(100, 98, 10), (90..100, 8));
    }

    #[test]
    fn the_config_keeps_its_source_through_a_dump() {
        let config = SourceBrowserConfig {
            chrome: PanelChrome {
                title: Some("Mine".into()),
                ..PanelChrome::default()
            },
            source: "plugin:example-tones".into(),
            ..SourceBrowserConfig::default()
        };

        let info = rox_dock::PanelInfo::panel(serde_json::to_value(&config).unwrap());
        let back: SourceBrowserConfig = panel::config_from_info(&info);

        assert_eq!(back.source, "plugin:example-tones");
        assert_eq!(back.chrome.title.as_deref(), Some("Mine"));
    }

    #[test]
    fn a_layout_without_a_source_opens_the_picker() {
        let info = rox_dock::PanelInfo::panel(serde_json::json!({}));
        let back: SourceBrowserConfig = panel::config_from_info(&info);

        assert!(back.source.is_empty());
    }

    #[test]
    fn the_librarys_top_is_no_plugin_root_and_keeps_its_flag_down_the_trail() {
        let top = Place::library();
        assert!(!top.is_root(), "the home doesn't follow the library's top");
        assert_eq!(top.node(), None);

        let kept = top.opening(Crumb {
            id: "liked".into(),
            title: "Liked".into(),
            query: None,
            collection: true,
            kind: None,
        });
        assert!(kept.library);
        assert_eq!(kept.node().map(|crumb| crumb.id.as_str()), Some("liked"));
    }

    #[test]
    fn search_results_list_no_node_even_as_a_crumb() {
        let results = Place {
            trail: vec![Crumb {
                id: "liked".into(),
                title: "Liked".into(),
                query: None,
                collection: true,
                kind: None,
            }],
            query: Some("tones".into()),
            view: None,
            library: false,
        };
        assert_eq!(results.node(), None, "search results list no node");
        assert!(!results.is_root());
        assert!(Place::default().is_root());

        let left = Place {
            trail: vec![Crumb {
                id: String::new(),
                title: "tones".into(),
                query: Some("tones".into()),
                collection: false,
                kind: None,
            }],
            ..Place::default()
        };
        assert_eq!(left.node(), None, "a results crumb isn't a node to list");
    }

    #[test]
    fn a_tiles_section_draws_its_run_as_one_shelf() {
        let section = |tiles| Entry::Section {
            title: "s".into(),
            tiles,
        };
        let entries = vec![
            section(true),
            node("a"),
            node("b"),
            section(false),
            node("c"),
            section(true),
            section(true),
            node("d"),
        ];

        assert_eq!(
            layout(&entries, 4),
            vec![
                Visual::Row(0),
                Visual::Shelf(1..3),
                Visual::Row(3),
                Visual::Row(4),
                Visual::Row(5),
                Visual::Row(6),
                Visual::Shelf(7..8),
            ]
        );
        assert_eq!(visual_of(&layout(&entries, 4), 2), Some(1));
        assert_eq!(visual_of(&layout(&entries, 4), 4), Some(3));
    }

    #[test]
    fn a_page_of_one_tiles_section_wraps_into_lines() {
        let entries = vec![
            Entry::Section {
                title: "s".into(),
                tiles: true,
            },
            node("a"),
            node("b"),
            node("c"),
            node("d"),
            node("e"),
        ];

        let visuals = layout(&entries, 2);
        assert_eq!(
            visuals,
            vec![
                Visual::Row(0),
                Visual::Line(1..3),
                Visual::Line(3..5),
                Visual::Line(5..6),
            ]
        );
        assert_eq!(visual_of(&visuals, 4), Some(2));
    }
}
