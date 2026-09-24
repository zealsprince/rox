//! The station directory: radio-browser.info in a window of its own. The
//! stations panel holds the list you keep; this is where it grows from.
//!
//! A hit lands through [`rox_library::stations::put`], the same write a typed
//! URL takes. The one extra is the hit's favicon, fetched best effort into
//! the thumbs database under the station's URL, silently, since a dead logo
//! link isn't a failed add.
//!
//! Nothing here removes or edits: the directory is read-only, and a kept
//! station is the panel's business.

use std::collections::HashSet;

use gpui::{
    AnyElement, App, Bounds, Context, Div, Entity, FocusHandle, Focusable as _, Global,
    KeyDownEvent, SharedString, Subscription, Window, WindowHandle, div, prelude::*, px, size,
};
use gpui_component::button::Button;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Icon, Root, Sizable as _};

use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_library::cue::{TrackKey, source_id};
use rox_library::stations::{self, Station};
use rox_net::sources::radio_browser::{self, Found};
use rox_panel_api::panel::{self, AppState};
use rox_services::catalog::LibraryEvent;

/// The directory ranks by votes, so past the head of the list is noise.
const RESULT_LIMIT: usize = 50;

const DEFAULT_SIZE: (f32, f32) = (560., 680.);

const MIN: gpui::Size<gpui::Pixels> = gpui::Size {
    width: px(420.),
    height: px(320.),
};

struct OpenDirectory(WindowHandle<Root>);

impl Global for OpenDirectory {}

pub fn open(state: AppState, cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenDirectory>() {
        let handle = open.0;
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }

    let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
    let handle = rox_panel_api::panel::open_child_window(
        cx,
        rox_i18n::t!("directory-window-title"),
        bounds,
        Some(MIN),
        move |window, cx| cx.new(|cx| StationDirectory::new(state, window, cx)),
    );

    cx.set_global(OpenDirectory(handle));
}

struct StationDirectory {
    state: AppState,
    find: Entity<InputState>,
    found: Vec<Found>,
    /// None until a search has come back.
    found_for: Option<String>,
    searching: bool,
    /// Bumped per search, so a slow earlier reply can't land over a newer one.
    search_generation: u64,
    failed: Option<SharedString>,
    held: HashSet<String>,
    focus: FocusHandle,
    _library_changed: Subscription,
    _find_events: Subscription,
}

impl StationDirectory {
    fn new(state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let find = cx.new(|cx| {
            InputState::new(window, cx).placeholder(rox_i18n::t!("directory-placeholder"))
        });
        window.focus(&find.focus_handle(cx));

        let _find_events = cx.subscribe_in(
            &find,
            window,
            |this: &mut Self, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.search(cx);
                }
            },
        );

        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Updated) {
                    this.refresh(cx);
                }
            },
        );

        let mut this = StationDirectory {
            state,
            find,
            found: Vec::new(),
            found_for: None,
            searching: false,
            search_generation: 0,
            failed: None,
            held: HashSet::new(),
            focus: cx.focus_handle(),
            _library_changed,
            _find_events,
        };
        this.refresh(cx);
        this
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.held = self
            .open_db(cx)
            .and_then(|conn| stations::all(&conn).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|station| station.url)
            .collect();

        cx.notify();
    }

    /// Opened per call rather than held, so it doesn't sit through every scan.
    fn open_db(&self, cx: &App) -> Option<rox_library::rusqlite::Connection> {
        let path = self.state.library.read(cx).db_path();

        rox_library::store::open(&path).ok()
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        let text = self.find.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }

        self.search_generation += 1;
        let generation = self.search_generation;
        self.searching = true;
        self.failed = None;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let query = text.clone();
            let result = cx
                .background_executor()
                .spawn(async move { radio_browser::search(&query, RESULT_LIMIT) })
                .await;

            this.update(cx, |this, cx| {
                if this.search_generation != generation {
                    return;
                }

                this.searching = false;
                match result {
                    Ok(found) => {
                        this.found = found;
                        this.found_for = Some(text);
                    }
                    Err(reason) => {
                        this.found.clear();
                        this.found_for = None;
                        this.failed = Some(rox_i18n::t!("directory-failed", reason = reason));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn add(&mut self, ix: usize, play: bool, cx: &mut Context<Self>) {
        let Some(hit) = self.found.get(ix).cloned() else {
            return;
        };

        let station = station_from(&hit);
        if !self.write(&station, cx) {
            return;
        }

        self.cache_favicon(&hit, cx);

        if play {
            let key = key_for(&station.url);
            self.state
                .player
                .update(cx, |player, cx| player.play_now(vec![key], cx));
        }
    }

    /// Rebuild the projection so the row shows up everywhere, not just the panel.
    fn write(&mut self, station: &Station, cx: &mut Context<Self>) -> bool {
        let Some(mut conn) = self.open_db(cx) else {
            return false;
        };
        if let Err(e) = stations::put(&mut conn, std::slice::from_ref(station)) {
            log::warn!("station directory: writing {} failed: {e}", station.url);
            return false;
        }

        self.state
            .library
            .update(cx, |library, cx| library.reload_projection(cx));
        self.refresh(cx);
        true
    }

    fn cache_favicon(&self, hit: &Found, cx: &mut Context<Self>) {
        let url = hit.favicon.trim().to_string();
        if url.is_empty() {
            return;
        }

        let Some(conn) = self.state.thumbs.read(cx).store_conn() else {
            return;
        };
        let key = hit.url.clone();
        let thumbs = self.state.thumbs.clone();

        // The row may already have painted and cached "no art" as definitive, so
        // forget that answer once the logo is filed.
        cx.spawn(async move |_, cx| {
            let stored = cx
                .background_executor()
                .spawn(async move {
                    rox_services::station_art::fetch_and_store(&url, &key, &conn).then_some(key)
                })
                .await;

            let Some(key) = stored else {
                return;
            };

            thumbs
                .update(cx, |thumbs, cx| {
                    thumbs.forget(std::path::Path::new(&key), cx);
                })
                .ok();
        })
        .detach();
    }

    fn search_row(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(tokens::SPACE_XS)
            .p(tokens::SPACE_SM)
            .border_b_1()
            .border_color(palette::border())
            .child(div().flex_1().min_w_0().child(Input::new(&self.find)))
            .child(
                Button::new("directory-search")
                    .icon(Icon::default().path(icons::SEARCH))
                    .tooltip(rox_i18n::t!("directory-search"))
                    .on_click(cx.listener(|this, _, _, cx| this.search(cx))),
            )
    }

    fn results(&self, cx: &mut Context<Self>) -> Div {
        let centered = |line: SharedString| {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .p(tokens::SPACE_MD)
                .text_sm()
                .text_center()
                .text_color(palette::text_muted())
                .child(line)
        };

        if self.searching {
            return centered(rox_i18n::t!("directory-searching"));
        }

        if let Some(failed) = self.failed.clone() {
            return centered(failed);
        }

        if self.found.is_empty() {
            let Some(text) = self.found_for.clone() else {
                return div().flex_1();
            };

            return centered(rox_i18n::t!("directory-none", text = text));
        }

        let rows: Vec<AnyElement> = self
            .found
            .iter()
            .enumerate()
            .map(|(ix, hit)| self.result_row(ix, hit, cx))
            .collect();

        div().flex_1().min_h_0().w_full().flex().flex_col().child(
            div()
                .id("directory-results")
                .size_full()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .children(rows),
        )
    }

    fn result_row(&self, ix: usize, hit: &Found, cx: &mut Context<Self>) -> AnyElement {
        let actions: AnyElement = if self.held.contains(&hit.url) {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(tokens::SPACE_XS)
                .text_xs()
                .text_color(palette::text_muted())
                .child(Icon::default().path(icons::CHECK))
                .child(rox_i18n::t!("directory-added"))
                .into_any_element()
        } else {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(tokens::SPACE_XS)
                .child(
                    Button::new(("directory-add", ix))
                        .icon(Icon::default().path(icons::PLUS))
                        .label(rox_i18n::t!("directory-add"))
                        .small()
                        .outline()
                        .on_click(cx.listener(move |this, _, _, cx| this.add(ix, false, cx))),
                )
                .child(
                    Button::new(("directory-add-play", ix))
                        .icon(Icon::default().path(icons::PLAY))
                        .label(rox_i18n::t!("directory-add-play"))
                        .small()
                        .outline()
                        .on_click(cx.listener(move |this, _, _, cx| this.add(ix, true, cx))),
                )
                .into_any_element()
        };

        div()
            .id(("directory-hit", ix))
            .flex()
            .items_center()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_SM)
            .py(tokens::SPACE_XS)
            .w_full()
            .min_w_0()
            .hover(|row| row.bg(palette::bg_control_hover()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .truncate()
                            .text_sm()
                            .text_color(palette::text())
                            .child(SharedString::from(hit.name.clone())),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(SharedString::from(meta_line(hit))),
                    ),
            )
            .child(actions)
            .into_any_element()
    }
}

impl Render for StationDirectory {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);

        panel::window_body(player, || {
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                .track_focus(&self.focus)
                // The box passes an idle escape through, so this catches it anywhere.
                .on_key_down(cx.listener(|_, event: &KeyDownEvent, window, _| {
                    if event.keystroke.key == "escape" {
                        window.remove_window();
                    }
                }))
                .child(self.search_row(cx))
                .child(self.results(cx))
                .into_any_element()
        })
    }
}

/// The first tag becomes the genre, the only one a row has a column for.
fn station_from(hit: &Found) -> Station {
    Station {
        url: hit.url.clone(),
        name: hit.name.clone(),
        genre: hit
            .tags
            .split(',')
            .map(str::trim)
            .find(|tag| !tag.is_empty())
            .unwrap_or_default()
            .to_string(),
    }
}

fn meta_line(hit: &Found) -> String {
    let mut parts: Vec<String> = Vec::new();

    if !hit.country.is_empty() {
        parts.push(hit.country.clone());
    }
    if !hit.codec.is_empty() {
        parts.push(hit.codec.clone());
    }
    if hit.bitrate_kbps > 0 {
        parts.push(rox_i18n::t!("directory-bitrate", kbps = hit.bitrate_kbps).to_string());
    }

    parts.join(", ")
}

/// The same shape [`rox_library::stations`] writes its rows with, which is
/// what makes the resolve find them.
fn key_for(url: &str) -> TrackKey {
    TrackKey {
        source: source_id(stations::SOURCE),
        path: url.into(),
        sub: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(tags: &str, country: &str, codec: &str, kbps: u16) -> Found {
        Found {
            name: "Deep Space One".into(),
            url: "https://ice2.somafm.com/deepspaceone-128-aac".into(),
            tags: tags.into(),
            country: country.into(),
            codec: codec.into(),
            bitrate_kbps: kbps,
            ..Found::default()
        }
    }

    #[test]
    fn a_hit_becomes_a_station_with_its_first_tag_as_genre() {
        let station = station_from(&hit(" ambient, space, drone", "US", "AAC", 128));

        assert_eq!(station.url, "https://ice2.somafm.com/deepspaceone-128-aac");
        assert_eq!(station.name, "Deep Space One");
        assert_eq!(station.genre, "ambient");
        assert_eq!(station_from(&hit("", "", "", 0)).genre, "");
    }

    #[test]
    fn the_meta_line_skips_what_the_directory_does_not_know() {
        assert_eq!(meta_line(&hit("", "US", "AAC", 128)), "US, AAC, 128 kbps");
        assert_eq!(meta_line(&hit("", "", "MP3", 0)), "MP3");
        assert_eq!(meta_line(&hit("", "", "", 0)), "");
    }

    #[test]
    fn a_station_plays_under_the_radio_source() {
        let key = key_for("https://host/jazz");

        assert_eq!(&*key.source, stations::SOURCE);
        assert_eq!(key.path.to_string_lossy(), "https://host/jazz");
        assert_eq!(key.sub, 0);
        assert!(!key.is_local(), "a station is never a file on disk");
    }
}
