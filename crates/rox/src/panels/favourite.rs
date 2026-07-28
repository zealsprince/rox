//! The favourite panel: a heart over one track, filled while that track
//! sits in the favourites playlist. Which track is per-view config through
//! [`crate::source::TrackSource`] - the playing one by default, or the
//! library selection - so a duplicate can watch each. A click runs the same
//! catalog toggle the library table's heart column does, so the state
//! matches wherever else it shows.

use std::path::PathBuf;

use gpui::{
    div, prelude::*, px, svg, AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable,
    MouseButton, Pixels, SharedString, Subscription, WeakEntity, Window,
};
use gpui_component::menu::PopupMenu;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{self, align_row, justify, Align, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::panels::library::LibraryEvent;
use crate::selection::SelectionEvent;
use crate::source::{self, ResolvedTrack, TrackSource};

/// The favourite panel's per-view config: what a saved layout restores, and
/// what the settings window edits.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct FavouriteConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub source: TrackSource,
    #[serde(default)]
    pub align: Align,
}

/// The track the heart currently sits over: the path it was resolved from,
/// that path's catalog id (None for a file the library does not know), and
/// whether the id is favourited. Cached so the pump's per-frame notifies
/// never turn into database lookups.
struct Tracked {
    path: PathBuf,
    id: Option<i64>,
    favourite: bool,
}

pub struct FavouritePanel {
    state: AppState,
    config: FavouriteConfig,
    /// The cached source resolve, so a selection lookup only runs after the
    /// selection or the catalog moves.
    resolved: ResolvedTrack,
    track: Option<Tracked>,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
    _selection_changed: Subscription,
    _library_changed: Subscription,
}

impl FavouritePanel {
    pub fn new(state: AppState, config: FavouriteConfig, cx: &mut Context<Self>) -> Self {
        // The shown track only turns over when the playing one does, so ride
        // the gated observe rather than the pump's per-tick notify.
        let _player_changed = crate::player::observe_view(&state.player, cx);
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, _: &SelectionEvent, cx| {
                this.resolved.invalidate();
                cx.notify();
            },
        );
        // The heart moves on any playlist change, this panel's own click
        // included; a rescan can rewrite the id -> path mapping under it, so
        // that drops the whole cache.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| match event {
                LibraryEvent::PlaylistsChanged => this.refresh_favourite(cx),
                LibraryEvent::Updated => {
                    this.resolved.invalidate();
                    this.track = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        FavouritePanel {
            state,
            config,
            resolved: ResolvedTrack::default(),
            track: None,
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
            _selection_changed,
            _library_changed,
        }
    }

    /// The shown track's id and favourite state, resolving and caching on a
    /// path change. No id while the source points at nothing, or at a file
    /// the library does not carry.
    fn current(&mut self, cx: &App) -> (Option<i64>, bool) {
        let Some(path) = self.resolved.get(self.config.source, &self.state, cx) else {
            self.track = None;
            return (None, false);
        };
        if self.track.as_ref().map(|t| &t.path) != Some(&path) {
            let library = self.state.library.read(cx);
            let id = library.id_for(&path);
            let favourite = id.is_some_and(|id| library.is_favourite(id));
            self.track = Some(Tracked {
                path,
                id,
                favourite,
            });
        }
        self.track
            .as_ref()
            .map_or((None, false), |t| (t.id, t.favourite))
    }

    /// Re-read the shown track's favourite state after a playlist change,
    /// here or on another surface. The id stays put, so this costs one
    /// single-track query rather than a resolve.
    fn refresh_favourite(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.track.as_ref().and_then(|t| t.id) else {
            return;
        };
        let favourite = self.state.library.read(cx).is_favourite(id);
        if let Some(track) = self.track.as_mut() {
            track.favourite = favourite;
        }
        cx.notify();
    }

    /// The panel's own dropdown entries: the source pick, the same knob the
    /// customize window edits.
    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        source::source_flyout(
            menu,
            |this: &Self| this.config.source,
            &cx.entity(),
            |this, source, cx| {
                this.config.source = source;
                cx.notify();
            },
            window,
            cx,
        )
    }

    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let (id, on) = self.current(cx);
        let heart = div()
            .size(px(24.))
            .rounded(tokens::RADIUS)
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path(if on {
                        icons::HEART_FILLED
                    } else {
                        icons::HEART
                    })
                    .size(px(15.))
                    .text_color(if on {
                        palette::accent()
                    } else {
                        palette::text_faint()
                    }),
            )
            // Nothing to favourite: the heart stays up, dimmed and dead, so
            // the panel holds its place in the strip.
            .when(id.is_none(), |d| d.opacity(0.4))
            .when_some(id, |d, id| {
                d.cursor_pointer()
                    .hover(|d| d.bg(palette::bg_control_hover()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this: &mut Self, _, _, cx| {
                            this.state
                                .library
                                .update(cx, |library, cx| library.set_favourites(&[id], !on, cx));
                        }),
                    )
            });

        div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .items_center()
            .map(|d| justify(d, self.config.align))
            .px(tokens::SPACE_MD)
            .child(heart)
    }
}

impl PanelSettings for FavouritePanel {
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
        &[("Content", icons::HEART)]
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
            .child(source::source_row(
                self.config.source,
                |this: &mut Self, source, cx| {
                    this.config.source = source;
                    cx.notify();
                },
                cx,
            ))
            .child(align_row(
                self.config.align,
                |this: &mut Self, align, cx| {
                    this.config.align = align;
                    cx.notify();
                },
                cx,
            ))
            .into_any_element()
    }
}

impl Render for FavouritePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

impl EventEmitter<PanelEvent> for FavouritePanel {}

impl Focusable for FavouritePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for FavouritePanel {
    fn panel_name(&self) -> &'static str {
        "favourite"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Favourite")
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

    fn min_size(&self, _cx: &App) -> gpui::Size<Pixels> {
        // The heart plus the strip's padding, raised by any user floor, the
        // theme toggle's floor for the same shape of button.
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(px(48.), rox_dock::resizable::PANEL_MIN_SIZE),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<Pixels> {
        crate::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
    }

    /// The layout dump carries the panel's config; the builder registered
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

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        // The config block: the panel's quick entries and the settings
        // window, apart from the core panel items.
        let menu = self.config_menu(menu, window, cx);
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity(), cx);
        let menu = panel::duplicate_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            |this, _window, cx| {
                let (state, config) = {
                    let panel = this.read(cx);
                    (panel.state.clone(), panel.config.clone())
                };
                FavouritePanel::new(state, config, cx)
            },
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
        )
    }
}
