//! The stations panel: web radio, listed and played. A station is an
//! ordinary track row under the radio source (see
//! [`rox_library::stations`]), so there's no special play verb and no
//! second queue; everything downstream treats it as the track it is.
//!
//! Listing and playing is the whole of it. The directory window finds
//! stations and the Radio page in settings keeps the list. Rows pick the
//! way the library's do and publish on the app-wide selection, so the
//! status bar never shows another panel's pick over this list.

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

/// Travels as the settings window's nav key, since the panels are a
/// crate below that window.
const RADIO_PAGE: &str = "settings-page-radio";

/// One size for every row, so the pictures stay a column around the
/// playing station.
const ART: Pixels = px(40.);

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StationsConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
}

pub struct StationsPanel {
    state: AppState,
    config: StationsConfig,
    stations: Vec<(Station, Heard)>,
    playing: Option<TrackKey>,
    /// The whole second the clocks last read, so the 16ms pump doesn't
    /// repaint the list sixty times a second.
    clock_secs: u64,
    /// A station moving to the next song changes a row and nothing else.
    seen_rev: u64,
    /// A playlist of local files imports as nothing; say so rather than fail
    /// silently.
    notice: Option<SharedString>,
    /// None means the press missed the rows, and the menu is the panel's own.
    menu_row: Option<usize>,
    /// By URL: the list is re-read whole on every catalog change, so an index
    /// would drift.
    selected: HashSet<String>,
    anchor: Option<String>,
    /// The arrows step from here. A shift-click moves it and not the anchor,
    /// or shift plus down would re-pick the same two rows forever.
    cursor: Option<String>,
    /// Resolved when the list is read, so a click never queries per row.
    ids: HashMap<String, i64>,
    scroll: ScrollHandle,
    focus: FocusHandle,
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
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Updated) {
                    this.refresh(cx);
                }
            },
        );
        // Clocks read in whole seconds, so a playing station doesn't repaint the
        // list every tick.
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

        let _thumbs_changed = cx.observe(&state.thumbs, |_: &mut Self, _, cx| cx.notify());

        // Another panel's pick clears the marks here. The clear is local:
        // republishing would fight whoever just published.
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

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.stations = self
            .open_db(cx)
            .and_then(|conn| stations::detailed(&conn).ok())
            .unwrap_or_default();

        // Resolved at the list's cadence, not per click.
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

        // A removed station takes its mark with it.
        self.selected.retain(|url| self.ids.contains_key(url));
        self.anchor = self.anchor.take().filter(|url| self.ids.contains_key(url));
        self.cursor = self.cursor.take().filter(|url| self.ids.contains_key(url));

        cx.notify();
    }

    fn url_at(&self, ix: usize) -> Option<String> {
        self.stations
            .get(ix)
            .map(|(station, _)| station.url.clone())
    }

    fn index_of(&self, url: &str) -> Option<usize> {
        self.stations
            .iter()
            .position(|(station, _)| station.url == url)
    }

    fn selected_urls(&self) -> Vec<String> {
        self.stations
            .iter()
            .filter(|(station, _)| self.selected.contains(&station.url))
            .map(|(station, _)| station.url.clone())
            .collect()
    }

    /// Keyed on the URL, so a re-read of the list keeps the marks.
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
            // Ctrl+shift stacks the range onto the set.
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

    /// Every call publishes, an empty set included: that's how a pick here
    /// replaces one made in the library and how Escape hands the scope back.
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

    /// Opened per call, not held: edits here are human-paced, and a held
    /// connection would sit through every scan.
    fn open_db(&self, cx: &App) -> Option<rox_library::rusqlite::Connection> {
        let path = self.state.library.read(cx).db_path();

        rox_library::store::open(&path).ok()
    }

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
                // A playlist of local files imports as nothing. Say so.
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

    /// The same delete the Radio page makes, which asks nothing first.
    fn remove(&mut self, urls: &[String], cx: &mut Context<Self>) {
        if urls.is_empty() {
            return;
        }

        let Some(mut conn) = self.open_db(cx) else {
            return;
        };
        // One failed row doesn't hold up the rest.
        for url in urls {
            if let Err(e) = stations::remove(&mut conn, url) {
                log::warn!("stations: removing a station failed: {e}");
            }
        }

        self.state
            .library
            .update(cx, |library, cx| library.reload_projection(cx));
        // The refresh drops the removed rows' marks, so this publish narrows the
        // shared scope.
        self.refresh(cx);
        self.publish_selection(cx);
    }

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
            // One menu over the whole list. A menu per row shares one element state
            // across the rows and never opens.
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

        // A right press clears the target on the way down; a row's handler runs
        // after and sets it back.
        list.capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, _| {
            if event.button == MouseButton::Right {
                this.menu_row = None;
            }
        }))
    }

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
        // The homepage is only known for the playing station: it's read at
        // connect and never stored.
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
        // A station can join a static list; the rest of the track menu has
        // nothing to act on.
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
            .when(selected, |row| {
                row.bg(palette::alpha(palette::accent(), 0x26))
            })
            // The pick outranks the playing highlight, the library's order.
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
                    // The playing row's second line is the song; every other row's is the
                    // stream's facts.
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
            .tooltip(move |window, cx| Tooltip::new(url.clone()).build(window, cx))
            // The press picks, not the click, so the highlight is already right when
            // a right press opens the menu.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
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
            // The press keeps going so the list's handler sees it. A press outside
            // the set picks just that row, so the menu never acts on unseen picks.
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

            // Pending and Missing draw the same mark, so a station without a logo
            // still reads as a station.
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

    /// The song's clock only reads once the stream has named one. None before
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

    /// None except for the playing station: the header is read at connect
    /// and never stored.
    fn homepage(&self, url: &str, cx: &App) -> Option<String> {
        if self.playing.as_ref()?.path.to_string_lossy() != url {
            return None;
        }

        let homepage = self.state.player.read(cx).station_info()?.homepage;

        (!homepage.trim().is_empty()).then(|| homepage.trim().to_string())
    }

    fn find(&self, cx: &mut App) {
        rox_panel_api::openers::station_directory(self.state.clone(), cx);
    }
}

fn manage(window: &mut Window, cx: &mut App) {
    panel_settings::open_app_page(RADIO_PAGE, window, cx);
}

/// The shape [`rox_library::stations`] writes its rows with, which is
/// what makes the resolve find them.
fn key_for(url: &str) -> TrackKey {
    TrackKey {
        source: source_id(stations::SOURCE),
        path: url.into(),
        sub: 0,
    }
}

/// None when the row holds none of the three; a line of separators is
/// worse than no line.
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

fn song_text(artist: &str, title: &str) -> String {
    if artist.is_empty() {
        return title.to_string();
    }

    format!("{artist} - {title}")
}

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

    /// Find sits on the tab bar: an empty panel is waiting on the directory.
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
        // A `.pls` imports from here, where somebody looking at their stations
        // already is.
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

    #[test]
    fn a_station_plays_under_the_radio_source() {
        let key = key_for("https://host/jazz");

        assert_eq!(&*key.source, stations::SOURCE);
        assert_eq!(key.path.to_string_lossy(), "https://host/jazz");
        assert_eq!(key.sub, 0);
        assert!(!key.is_local(), "a station is never a file on disk");
    }

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

    #[test]
    fn the_song_line_survives_half_a_title() {
        assert_eq!(song_text("Miles Davis", "So What"), "Miles Davis - So What");
        assert_eq!(song_text("", "So What"), "So What");
    }
}
