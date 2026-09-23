//! The stations panel: web radio, listed and played. A station is an
//! ordinary track row under the radio source (see
//! [`rox_library::stations`]), so there's no special play verb here and no
//! second queue. A row resolves to a locator and goes to the player the
//! way a file does, and everything downstream (the queue, the history, the
//! visualizers) treats it as the track it is.
//!
//! Listing and playing is the whole of it. This panel used to carry an add
//! row, a directory search, and a results view standing in for the list,
//! which is three forms wedged into a surface people keep open to press
//! play. The directory window finds stations now, and the Radio page in
//! settings keeps the list. What's left here is the list, a double click
//! to play, and two ways over to the surfaces that own the rest.
//!
//! The rows pick the way the library's do: a click takes one, shift and
//! ctrl build a set, and the set publishes on the app-wide selection. That
//! last part is what makes the panel stop lying. Before it, a pick made in
//! the library went on standing in the status bar while you worked in here,
//! so the strip read somebody else's tracks over a list of stations. Play
//! and Remove act on the whole set; Remove is the Radio page's delete,
//! run once per row, and it asks nothing first because that one doesn't
//! either.
//!
//! A row shows everything known about a station, which for a long time was
//! a name and a URL and read as a bookmarks file. The logo comes out of
//! the thumbnail pool under the row's path, the genre, codec and bitrate
//! are what the stream said when it was last played, and the URL itself
//! moves to a tooltip: it's the identity, not something anyone reads.
//!
//! Two lines, always. The name holds the top one, with the clocks pinned to
//! its far end while the station plays; the second line is the song on air
//! for the playing row and the stream's own facts for every other. A
//! station is the one row in the app whose contents change while it sits
//! there, and trading the facts away for the song is what keeps that from
//! costing the list a line of height it only ever needs once.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use gpui::{
    AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable, KeyDownEvent, Modifiers,
    MouseButton, MouseDownEvent, ObjectFit, Pixels, ScrollHandle, SharedString, Stateful,
    Subscription, WeakEntity, Window, div, img, prelude::*, px, svg,
};
use gpui_component::Icon;
use gpui_component::button::Button;
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::tooltip::Tooltip;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::{TrackKey, source_id};
use rox_library::stations::{self, Heard, Station};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::player::fmt_time;
use crate::selection::SelectionEvent;
use crate::thumbs::Thumb;

/// The settings page the list is kept on, named by the key the settings
/// window lists it under. The panels are a crate below that window, so a
/// jump to one of its pages travels as its nav key.
const RADIO_PAGE: &str = "settings-page-radio";

/// The logo square on the left of a row. One size for every row, which is
/// also what every row is: two lines, whether the second one carries the
/// song or the stream's facts, so the column of pictures reads as a column
/// instead of stepping in and out around the playing station.
const ART: Pixels = px(40.);

/// The panel's per-view config. The stations themselves live in the
/// library, so a saved layout restores the shared chrome and nothing else.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StationsConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
}

pub struct StationsPanel {
    state: AppState,
    config: StationsConfig,
    /// Each station with whatever the stream filled in for it: the codec
    /// and bitrate columns a row only has once it's been played.
    stations: Vec<(Station, Heard)>,
    /// The station playing when the panel last drew, so its row highlights.
    playing: Option<TrackKey>,
    /// The whole second the playing row's clocks last read. The pump
    /// notifies every 16ms and these count in seconds, so this is what
    /// keeps the list off sixty repaints a second while a station plays.
    clock_secs: u64,
    /// The title revision the list last drew. A station moving to the next
    /// song changes a row without changing anything else the panel reads.
    seen_rev: u64,
    /// Why the last import did nothing, shown over the list. A playlist of
    /// local files is a real thing to pick by mistake, and failing
    /// silently reads as the button being broken.
    notice: Option<SharedString>,
    /// The row under the last right press, what the list's context menu
    /// builds for. None means the press missed the rows, and the menu is
    /// the panel's own.
    menu_row: Option<usize>,
    /// The picked rows, by stream URL. The list is re-read whole on every
    /// catalog change, so an index would name a different station after
    /// one; the URL is a station's identity and survives that.
    selected: HashSet<String>,
    /// Where a shift-click measures its range from.
    anchor: Option<String>,
    /// The row the arrows step from, which a shift-click moves and the
    /// anchor doesn't: without the pair, shift plus down would re-pick the
    /// same two rows forever.
    cursor: Option<String>,
    /// Each station's library id, resolved when the list is read. The
    /// shared selection speaks in ids, and a click can't afford a query
    /// per row to find them.
    ids: HashMap<String, i64>,
    /// The list's scroll box, so an arrow step that leaves the visible
    /// rows brings its row along.
    scroll: ScrollHandle,
    focus: FocusHandle,
    /// The tab panel that currently hosts this panel, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _library_changed: Subscription,
    _player_changed: Subscription,
    _thumbs_changed: Subscription,
    _selection_changed: Subscription,
}

impl StationsPanel {
    pub fn new(
        state: AppState,
        config: StationsConfig,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // A scan or any other catalog write can move the rows under the
        // list, the same reason every other library-backed panel re-reads
        // on this. An add from the settings page arrives this way too.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Updated) {
                    this.refresh(cx);
                }
            },
        );
        // Three things move a row here and all of them arrive on the pump:
        // the station under the cursor, the song it moved to, and its own
        // clock. The clock is read in whole seconds so a station playing
        // doesn't cost the list a repaint every tick.
        let _player_changed = cx.observe(&state.player, |this: &mut Self, _, cx| {
            let player = this.state.player.read(cx);
            let playing = player.now_playing().map(|now| now.key);
            let clock_secs = player
                .now_playing()
                .map(|now| now.position_secs as u64)
                .unwrap_or(0);
            let rev = player.title_rev().unwrap_or(0);

            if this.playing == playing && this.clock_secs == clock_secs && this.seen_rev == rev {
                return;
            }

            this.playing = playing;
            this.clock_secs = clock_secs;
            this.seen_rev = rev;
            cx.notify();
        });

        // A logo landing in the pool repaints the rows waiting on one, the
        // same subscription every other art-drawing panel keeps.
        let _thumbs_changed = cx.observe(&state.thumbs, |_: &mut Self, _, cx| cx.notify());

        // A pick made anywhere else takes the scope, and the marks here
        // come down with it. Rows left lit under a count that no longer
        // describes them is the confusion this panel was fixed for, read
        // the other way around. The clear is local: republishing would
        // fight whoever just published.
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, event: &SelectionEvent, cx| {
                if event.source == cx.entity().entity_id() || this.selected.is_empty() {
                    return;
                }

                this.selected.clear();
                this.anchor = None;
                this.cursor = None;
                cx.notify();
            },
        );

        let mut panel = StationsPanel {
            playing: state.player.read(cx).now_playing().map(|now| now.key),
            clock_secs: 0,
            seen_rev: 0,
            state,
            config,
            stations: Vec::new(),
            notice: None,
            menu_row: None,
            selected: HashSet::new(),
            anchor: None,
            cursor: None,
            ids: HashMap::new(),
            scroll: ScrollHandle::default(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            _library_changed,
            _player_changed,
            _thumbs_changed,
            _selection_changed,
        };
        panel.refresh(cx);
        panel
    }

    /// Re-read the station list off the library's database. At panel-open
    /// and edit cadence, never per frame.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.stations = self
            .open_db(cx)
            .and_then(|conn| stations::detailed(&conn).ok())
            .unwrap_or_default();

        // The shared selection speaks in library ids, so they're resolved
        // here, at the list's own cadence, rather than once per click.
        let library = self.state.library.read(cx);
        self.ids = self
            .stations
            .iter()
            .filter_map(|(station, _)| {
                library
                    .id_for_key(&key_for(&station.url))
                    .map(|id| (station.url.clone(), id))
            })
            .collect();

        // A station removed here or on the Radio page takes its mark
        // with it, so the set never names a row that isn't in the list.
        self.selected.retain(|url| self.ids.contains_key(url));
        self.anchor = self.anchor.take().filter(|url| self.ids.contains_key(url));
        self.cursor = self.cursor.take().filter(|url| self.ids.contains_key(url));

        cx.notify();
    }

    /// The stream URL of a row, the name everything here picks by.
    fn url_at(&self, ix: usize) -> Option<String> {
        self.stations
            .get(ix)
            .map(|(station, _)| station.url.clone())
    }

    /// Where a station sits in the list now, for the shift range and the
    /// arrow steps.
    fn index_of(&self, url: &str) -> Option<usize> {
        self.stations
            .iter()
            .position(|(station, _)| station.url == url)
    }

    /// The picked stations in list order, which is the order they play and
    /// the order the selection publishes them in.
    fn selected_urls(&self) -> Vec<String> {
        self.stations
            .iter()
            .filter(|(station, _)| self.selected.contains(&station.url))
            .map(|(station, _)| station.url.clone())
            .collect()
    }

    /// Put a click on a station row: plain picks just it, shift extends
    /// from the anchor over the rows between, ctrl (cmd on macOS)
    /// toggles. The library's and the queue's rules, keyed on the URL so
    /// a re-read of the list keeps the marks.
    fn select(&mut self, ix: usize, modifiers: Modifiers, cx: &mut Context<Self>) {
        let Some(url) = self.url_at(ix) else {
            return;
        };

        if modifiers.shift {
            let anchor_ix = self
                .anchor
                .as_deref()
                .and_then(|anchor| self.index_of(anchor))
                .unwrap_or(ix);
            let (lo, hi) = (anchor_ix.min(ix), anchor_ix.max(ix));
            let range = self.stations[lo..=hi]
                .iter()
                .map(|(station, _)| station.url.clone());
            // Ctrl+shift stacks the range onto the set, the queue's move,
            // so a second run can be picked without losing the first.
            if modifiers.secondary() {
                self.selected.extend(range);
            } else {
                self.selected = range.collect();
            }
            if self.anchor.is_none() {
                self.anchor = Some(url.clone());
            }
        } else if modifiers.secondary() {
            if !self.selected.insert(url.clone()) {
                self.selected.remove(&url);
            }
            self.anchor = Some(url.clone());
        } else {
            self.selected = HashSet::from([url.clone()]);
            self.anchor = Some(url.clone());
        }

        self.cursor = Some(url);
        self.publish_selection(cx);
        cx.notify();
    }

    /// Take the whole list, which is what Remove on the set is usually
    /// after.
    fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.stations.is_empty() {
            return;
        }

        self.selected = self
            .stations
            .iter()
            .map(|(station, _)| station.url.clone())
            .collect();
        self.anchor = self.url_at(0);
        self.cursor = self.anchor.clone();
        self.publish_selection(cx);
        cx.notify();
    }

    /// Drop the pick, handing the shared scope back to the whole catalog.
    fn deselect(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            return;
        }

        self.selected.clear();
        self.anchor = None;
        self.cursor = None;
        self.publish_selection(cx);
        cx.notify();
    }

    /// Publish the pick on the shared selection, which is what the status
    /// strip and every selection-following panel read. Every call goes
    /// through, an empty set included: that's how a pick made here
    /// replaces one made in the library, and how Escape hands the scope
    /// back. A station the catalog has no row for drops out.
    fn publish_selection(&self, cx: &mut Context<Self>) {
        let ids: Vec<i64> = self
            .selected_urls()
            .iter()
            .filter_map(|url| self.ids.get(url).copied())
            .collect();
        let source = cx.entity_id();

        self.state
            .selection
            .update(cx, |selection, cx| selection.set(ids, source, cx));
    }

    /// Step the pick by one row, extending from the anchor while shift is
    /// held. From a cold panel the first step lands on the first row, so
    /// the arrows work without a click first.
    fn step(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        if self.stations.is_empty() {
            return;
        }

        let last = self.stations.len() - 1;
        let target = match self.cursor.as_deref().and_then(|url| self.index_of(url)) {
            Some(ix) => ix.saturating_add_signed(delta).min(last),
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
        self.scroll.scroll_to_item(target);
    }

    /// Arrows move the pick and shift extends it, Enter plays it, Escape
    /// drops it, and ctrl/cmd+A takes the list. The library panel's keys,
    /// as far as a flat list of stations has use for them.
    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let modifiers = &event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();

        if modifiers.secondary() && key == "a" {
            self.select_all(cx);
            return;
        }

        match key {
            "up" => self.step(-1, modifiers.shift, cx),

            "down" => self.step(1, modifiers.shift, cx),

            "enter" => {
                let urls = self.selected_urls();
                self.play_urls(&urls, cx);
            }

            "escape" => self.deselect(cx),

            _ => {}
        }
    }

    /// The panel's own connection to the library database, the idiom the
    /// metadata panel and the library panel's own side reads already use.
    /// Opened per call rather than held, since an edit here happens at
    /// human pace and a held connection would sit through every scan.
    fn open_db(&self, cx: &App) -> Option<rox_library::rusqlite::Connection> {
        let path = self.state.library.read(cx).db_path();

        rox_library::store::open(&path).ok()
    }

    /// Import a `.pls` or `.m3u` of stream URLs, which is how most people
    /// already have their stations.
    fn import(&self, window: &mut Window, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });

        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                return;
            };

            let found = stations::import(&text);
            this.update(cx, |this, cx| {
                // A playlist of local files is a real thing to pick by
                // mistake, and it imports as nothing. Say so.
                if found.is_empty() {
                    this.notice = Some(rox_i18n::t!("stations-import-empty"));
                    cx.notify();
                    return;
                }

                if this.write(&found, cx) {
                    this.notice = None;
                }
            })
            .ok();
        })
        .detach();
    }

    /// Write stations, then have the library rebuild its projection so the
    /// new rows show up everywhere else too, not only in this list.
    fn write(&mut self, stations: &[Station], cx: &mut Context<Self>) -> bool {
        let Some(mut conn) = self.open_db(cx) else {
            return false;
        };
        if let Err(e) = stations::put(&mut conn, stations) {
            log::warn!("stations: writing {} rows failed: {e}", stations.len());
            return false;
        }

        self.state
            .library
            .update(cx, |library, cx| library.reload_projection(cx));
        self.refresh(cx);
        true
    }

    /// Drop stations, then rebuild the projection the way a write does:
    /// the rows have to leave the library everywhere, not only this list.
    /// The same delete the Radio page makes, which asks nothing first,
    /// so neither does this.
    fn remove(&mut self, urls: &[String], cx: &mut Context<Self>) {
        if urls.is_empty() {
            return;
        }

        let Some(mut conn) = self.open_db(cx) else {
            return;
        };
        // One failed row doesn't hold up the rest: the set was picked as
        // a set, and stopping halfway leaves the list looking arbitrary.
        for url in urls {
            if let Err(e) = stations::remove(&mut conn, url) {
                log::warn!("stations: removing a station failed: {e}");
            }
        }

        self.state
            .library
            .update(cx, |library, cx| library.reload_projection(cx));
        // The refresh drops the marks the removed rows held, so the
        // publish that follows narrows the shared scope to what's left.
        self.refresh(cx);
        self.publish_selection(cx);
    }

    /// Play stations, which goes through the same path any track does:
    /// each key resolves to a locator and the player opens it. A set
    /// replaces the queue with itself, the way playing one already does.
    fn play_urls(&self, urls: &[String], cx: &mut Context<Self>) {
        if urls.is_empty() {
            return;
        }

        let keys: Vec<TrackKey> = urls.iter().map(|url| key_for(url)).collect();
        self.state
            .player
            .update(cx, |player, cx| player.play_now(keys, cx));
    }

    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_root())
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.on_key(event, cx)))
            .children(self.notice.clone().map(|notice| {
                div()
                    .flex_none()
                    .w_full()
                    .p(tokens::SPACE_SM)
                    .border_b_1()
                    .border_color(palette::border())
                    .text_xs()
                    .text_color(palette::tone_warn())
                    .child(notice)
            }))
            // One menu over the whole list, built for whichever row the
            // press landed on, the shape every other list panel uses. A
            // menu per row shares one element state across the rows and
            // never opens.
            .child(self.list(window, cx).context_menu({
                let weak = cx.entity().downgrade();
                move |menu, window, cx| {
                    let Some(this) = weak.upgrade() else {
                        return menu;
                    };
                    this.update(cx, |this, cx| this.row_menu(menu, window, cx))
                }
            }))
    }

    /// The rows, or the empty state when there are none. Either way the
    /// element records where a right press landed, so the menu the body
    /// hangs on it knows whether it's for a row or for the panel.
    fn list(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let list = if self.stations.is_empty() {
            self.empty_state(cx)
        } else {
            let rows: Vec<AnyElement> = self
                .stations
                .iter()
                .enumerate()
                .map(|(ix, (station, heard))| self.row(ix, station, heard, cx))
                .collect();

            div().flex_1().min_h_0().w_full().flex().flex_col().child(
                div()
                    .id("stations-list")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .flex()
                    .flex_col()
                    .children(rows),
            )
        };

        // A right press clears the target on the way down; a row's own
        // handler runs after and sets it back, so by the time the menu
        // builds the target is the row under the pointer or nothing.
        list.capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
            if event.button == MouseButton::Right {
                this.menu_row = None;
            }
        }))
    }

    /// No stations yet: the two doors out, since neither the finding nor
    /// the keeping happens in here any more.
    fn empty_state(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(tokens::SPACE_SM)
            .p(tokens::SPACE_MD)
            .text_center()
            .child(div().text_lg().child(rox_i18n::t!("stations-empty-title")))
            .child(
                div()
                    .text_sm()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("stations-empty")),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_XS)
                    .child(crate::settings::ui::small_button(
                        rox_i18n::t!("stations-find"),
                        icons::SEARCH,
                        false,
                        cx.listener(|this, _, _, cx| this.find(cx)),
                    ))
                    .child(crate::settings::ui::small_button(
                        rox_i18n::t!("stations-manage"),
                        icons::SETTINGS,
                        false,
                        |_, window, cx| manage(window, cx),
                    )),
            )
    }

    /// The right-click menu: play, add to a playlist and remove, each
    /// acting on every picked station; then the panel's own items. The
    /// panel menu alone when the press missed the rows. The press itself
    /// already put the row under it in the pick, so what the menu builds
    /// for is always what's lit.
    fn row_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let Some(url) = self.menu_row.and_then(|ix| self.url_at(ix)) else {
            return self.dropdown_menu(menu, window, cx);
        };

        let urls = match self.selected_urls() {
            picked if picked.is_empty() => vec![url.clone()],

            picked => picked,
        };
        let count = urls.len() as u64;
        let ids: Vec<i64> = urls
            .iter()
            .filter_map(|url| self.ids.get(url).copied())
            .collect();
        // A homepage only exists for the playing station: the header is
        // read at the connect and nothing stores it. It's a single row's
        // action either way, so a multi-row pick doesn't offer it.
        let homepage = (urls.len() == 1).then(|| self.homepage(&url, cx)).flatten();
        let play_urls = urls.clone();
        let remove_urls = urls;
        let play_panel = cx.entity().downgrade();
        let remove_panel = cx.entity().downgrade();

        let play_label = match count {
            0 | 1 => rox_i18n::t!("stations-play"),

            n => rox_i18n::t!("stations-play-count", count = n),
        };
        let remove_label = match count {
            0 | 1 => rox_i18n::t!("stations-remove"),

            n => rox_i18n::t!("stations-remove-count", count = n),
        };

        let menu = menu.item(
            PopupMenuItem::new(play_label)
                .icon(Icon::default().path(icons::PLAY))
                .on_click(move |_, _, cx| {
                    play_panel
                        .update(cx, |this, cx| this.play_urls(&play_urls, cx))
                        .ok();
                }),
        );
        // A station has a row in the library like anything else, so it
        // can join a static list. The rest of the shared track menu (tags,
        // renames, conversions) has nothing to act on here.
        let menu = panel::playlist_item(menu, self.state.clone(), ids, window, cx);
        let menu = menu.item(
            PopupMenuItem::new(remove_label)
                .icon(Icon::default().path(icons::TRASH))
                .on_click(move |_, _, cx| {
                    remove_panel
                        .update(cx, |this, cx| this.remove(&remove_urls, cx))
                        .ok();
                }),
        );

        let menu = match homepage {
            Some(homepage) => menu.item(
                PopupMenuItem::new(rox_i18n::t!("stations-homepage"))
                    .icon(Icon::default().path(icons::EXTERNAL_LINK))
                    .on_click(move |_, _, cx| cx.open_url(&homepage)),
            ),

            None => menu,
        };

        self.dropdown_menu(menu.separator(), window, cx)
    }

    /// One station: its logo, its name, and a line of whatever is known
    /// about the stream, with the URL moved to the tooltip. The playing
    /// row trades that second line for the song on air and pins the clocks
    /// beside the name. A click picks the row, shift and ctrl build a set
    /// out of it, a double click plays, and the right-click menu acts on
    /// whatever is picked.
    fn row(
        &self,
        ix: usize,
        station: &Station,
        heard: &Heard,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on_air = self
            .playing
            .as_ref()
            .is_some_and(|key| key.path.to_string_lossy() == station.url);

        let url = station.url.clone();
        let play_url = station.url.clone();
        let selected = self.selected.contains(&station.url);
        let facts = facts(heard);

        div()
            .id(("station-row", ix))
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .w_full()
            .min_w_0()
            .cursor_pointer()
            // The same accent wash the library and the queue mark a picked
            // row with, so a set reads the same everywhere.
            .when(selected, |row| {
                row.bg(palette::alpha(palette::accent(), 0x26))
            })
            // The playing row takes the highlight role, the same faint cut
            // the library and the bookmarks list pick a playing track out
            // with. The pick outranks it, the library's order too.
            .when(on_air && !selected, |row| {
                row.bg(palette::alpha(palette::highlight(), 0x12))
            })
            .hover(|row| row.bg(palette::bg_control_hover()))
            .child(self.art(&station.url, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
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
                                    .truncate()
                                    .text_sm()
                                    .text_color(if on_air {
                                        palette::text_bright()
                                    } else {
                                        palette::text()
                                    })
                                    .child(SharedString::from(display_name(station))),
                            )
                            .children(on_air.then(|| self.clocks(cx)).flatten()),
                    )
                    // The playing row's second line is the song, which
                    // moves; every other row's is the stream's facts,
                    // which don't. One line either way.
                    .children(match on_air {
                        true => Some(self.song_line(cx)),

                        false => facts.map(|facts| {
                            div()
                                .truncate()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .child(SharedString::from(facts))
                        }),
                    }),
            )
            // The URL is the station's identity, not something anyone
            // reads off a list, so it's here rather than on the row.
            .tooltip(move |window, cx| Tooltip::new(url.clone()).build(window, cx))
            // The press picks, not the click: that's what the library and
            // the queue do, and it means the highlight is already right
            // when a right press opens the menu on top of it.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    // Take focus so the arrows and Enter reach the panel's
                    // key handler.
                    window.focus(&this.focus);
                    if event.click_count == 1 {
                        this.select(ix, event.modifiers, cx);
                    }
                }),
            )
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                if event.click_count() >= 2 {
                    this.play_urls(std::slice::from_ref(&play_url), cx);
                }
            }))
            // Mark the row for the list's menu; the press itself keeps
            // going so the list's own handler sees it too. A press outside
            // the set picks just that row first, so the menu never acts on
            // rows the user can't see are picked.
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    this.menu_row = Some(ix);
                    let outside = this
                        .url_at(ix)
                        .is_none_or(|url| !this.selected.contains(&url));
                    if outside {
                        window.focus(&this.focus);
                        this.select(ix, Modifiers::default(), cx);
                    }
                }),
            )
            .into_any_element()
    }

    /// The station's picture out of the thumbnail pool, keyed by the row's
    /// path the way every other art lookup in the app is. A station added
    /// through the directory has its logo there already; one that was
    /// typed in gets it the first time it plays and names a homepage.
    fn art(&self, url: &str, cx: &mut Context<Self>) -> AnyElement {
        let thumb = self
            .state
            .thumbs
            .update(cx, |thumbs, cx| thumbs.get(Path::new(url), cx));

        match thumb {
            Thumb::Ready(image) => img(image)
                .flex_none()
                .size(ART)
                .overflow_hidden()
                .object_fit(ObjectFit::Cover)
                .rounded(tokens::RADIUS)
                .into_any_element(),

            // Pending and Missing draw the same mark. A logo that arrives
            // replaces it, and one that never does leaves a row that still
            // reads as a station rather than as a hole.
            _ => div()
                .flex_none()
                .size(ART)
                .flex()
                .items_center()
                .justify_center()
                .rounded(tokens::RADIUS)
                .bg(palette::bg_control())
                .child(
                    svg()
                        .path(icons::RADIO)
                        .size(px(18.))
                        .text_color(palette::text_faint()),
                )
                .into_any_element(),
        }
    }

    /// The playing row's second line: what the station is playing. Nothing
    /// is invented before the first announcement, which on some stations
    /// never comes.
    fn song_line(&self, cx: &App) -> Div {
        let song = self
            .state
            .player
            .read(cx)
            .live_over(None)
            .map(|meta| song_text(&meta.artist, &meta.title))
            .unwrap_or_else(|| rox_i18n::t!("stations-on-air").to_string());

        div()
            .truncate()
            .text_xs()
            .text_color(palette::accent())
            .child(SharedString::from(song))
    }

    /// The playing row's clocks, pinned to the end of the name line: how
    /// long this song has been on and how long the station has. The
    /// station's own clock always reads; the song's only reads once the
    /// stream has named one, which is what gives it a start. None before
    /// the position clock resolves.
    fn clocks(&self, cx: &App) -> Option<Stateful<Div>> {
        let player = self.state.player.read(cx);
        let station_secs = player.now_playing().map(|now| now.position_secs)?;
        let clocks = match player.song_elapsed() {
            Some(song_secs) => format!("{} / {}", fmt_time(song_secs), fmt_time(station_secs)),

            None => fmt_time(station_secs),
        };

        Some(
            div()
                .id("station-clocks")
                .flex_none()
                .text_xs()
                .text_color(palette::text_muted())
                .tooltip(|window, cx| {
                    Tooltip::new(rox_i18n::t!("stations-clocks")).build(window, cx)
                })
                .child(SharedString::from(clocks)),
        )
    }

    /// The homepage the playing station named in its headers, for the row
    /// that station is. None for every other row: the header is read at the
    /// connect and nothing stores it, so a station that isn't playing has
    /// no homepage anybody here knows about.
    fn homepage(&self, url: &str, cx: &App) -> Option<String> {
        if self.playing.as_ref()?.path.to_string_lossy() != url {
            return None;
        }

        let homepage = self.state.player.read(cx).station_info()?.homepage;

        (!homepage.trim().is_empty()).then(|| homepage.trim().to_string())
    }

    /// The directory window, where a station is searched for and added.
    fn find(&self, cx: &mut App) {
        rox_panel_api::openers::station_directory(self.state.clone(), cx);
    }
}

/// The settings window on its Radio page, where the list is kept.
fn manage(window: &mut Window, cx: &mut App) {
    panel_settings::open_app_page(RADIO_PAGE, window, cx);
}

/// The key a station plays under: the radio source, the stream URL as the
/// path, and no subsong. The same shape [`rox_library::stations`] writes
/// its rows with, which is what makes the resolve find them.
fn key_for(url: &str) -> TrackKey {
    TrackKey {
        source: source_id(stations::SOURCE),
        path: url.into(),
        sub: 0,
    }
}

/// The row's second line: the genre, the codec and the bitrate, whichever
/// of them the row actually holds. None when it holds none, which is a
/// station that has never been played and came from nowhere that knew
/// anything about it; a line of separators standing in for three missing
/// values is worse than no line.
fn facts(heard: &Heard) -> Option<String> {
    let bitrate = match heard.bitrate_kbps {
        0 => String::new(),

        kbps => rox_i18n::t!("directory-bitrate", kbps = kbps).to_string(),
    };

    let facts: Vec<&str> = [heard.genre.as_str(), heard.codec.as_str(), bitrate.as_str()]
        .into_iter()
        .filter(|fact| !fact.is_empty())
        .collect();

    (!facts.is_empty()).then(|| facts.join(", "))
}

/// The song on air as one line. A station that sends one unsplittable
/// field leaves the artist empty, and the title alone is then the whole of
/// what it said.
fn song_text(artist: &str, title: &str) -> String {
    if artist.is_empty() {
        return title.to_string();
    }

    format!("{artist} - {title}")
}

/// What a row shows for a station with no name of its own. The list is
/// sorted by name, so a blank one would sit at the top reading as nothing
/// at all.
fn display_name(station: &Station) -> String {
    if station.name.trim().is_empty() {
        station.url.clone()
    } else {
        station.name.clone()
    }
}

impl PanelSettings for StationsPanel {
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
}

impl EventEmitter<PanelEvent> for StationsPanel {}

impl Focusable for StationsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for StationsPanel {
    fn panel_name(&self) -> &'static str {
        "stations"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("stations-title"),
        )
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

    /// The layout dump stores the panel's config; the builder registered
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

    /// Find sits on the tab bar because it's the one thing here worth
    /// finding without a menu: a panel with no stations in it is waiting
    /// on the directory.
    fn toolbar_buttons(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        Some(vec![
            Button::new("stations-find")
                .icon(Icon::default().path(icons::SEARCH))
                .tooltip(rox_i18n::t!("stations-find"))
                .on_click(cx.listener(|this, _, _, cx| this.find(cx))),
        ])
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        // The list is kept in settings, and a `.pls` still imports from
        // here, since the panel is where somebody looking at their
        // stations already is.
        let menu = menu
            .item(
                PopupMenuItem::new(rox_i18n::t!("stations-manage"))
                    .icon(Icon::default().path(icons::SETTINGS))
                    .on_click(move |_, window, cx| manage(window, cx)),
            )
            .item({
                let panel = cx.entity().downgrade();
                PopupMenuItem::new(rox_i18n::t!("stations-import"))
                    .icon(Icon::default().path(icons::DOWNLOAD))
                    .on_click(move |_, window, cx| {
                        panel.update(cx, |this, cx| this.import(window, cx)).ok();
                    })
            })
            .separator();

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
                StationsPanel::new(state, config, window, cx)
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

impl Render for StationsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn station(url: &str, name: &str) -> Station {
        Station {
            url: url.to_string(),
            name: name.to_string(),
            genre: String::new(),
        }
    }

    /// A station's key names the radio source and carries the stream URL
    /// where a file would carry its path. That's the whole play path: no
    /// special verb, just a key the resolve knows how to look up.
    #[test]
    fn a_station_plays_under_the_radio_source() {
        let key = key_for("https://host/jazz");

        assert_eq!(&*key.source, stations::SOURCE);
        assert_eq!(key.path.to_string_lossy(), "https://host/jazz");
        assert_eq!(key.sub, 0);
        assert!(!key.is_local(), "a station is never a file on disk");
    }

    /// A row never draws blank. An unnamed station shows its URL, which is
    /// the only thing it has.
    #[test]
    fn an_unnamed_station_shows_its_url() {
        assert_eq!(
            display_name(&station("https://host/jazz", "")),
            "https://host/jazz"
        );
        assert_eq!(
            display_name(&station("https://host/jazz", " ")),
            "https://host/jazz"
        );
        assert_eq!(display_name(&station("https://host/jazz", "Jazz")), "Jazz");
    }

    /// The second line is whatever the row actually holds. A station with
    /// nothing filled in yet has no second line at all, rather than a row
    /// of commas standing in for what nobody knows.
    #[test]
    fn the_second_line_carries_only_what_is_known() {
        assert_eq!(facts(&Heard::default()), None);

        assert_eq!(
            facts(&Heard {
                genre: "Jazz".into(),
                codec: "mp3".into(),
                bitrate_kbps: 128,
            })
            .as_deref(),
            Some("Jazz, mp3, 128 kbps")
        );

        assert_eq!(
            facts(&Heard {
                codec: "aac".into(),
                ..Heard::default()
            })
            .as_deref(),
            Some("aac")
        );
    }

    /// A station that sends one unsplittable field leaves the artist
    /// empty, and the title alone is then the whole announcement.
    #[test]
    fn the_song_line_survives_half_a_title() {
        assert_eq!(song_text("Miles Davis", "So What"), "Miles Davis - So What");
        assert_eq!(song_text("", "So What"), "So What");
    }
}
