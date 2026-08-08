//! The rating panel: one track's stars, or the numeric readout when the
//! app-wide rating style says so, as a control of its own. Which track is
//! per-view config through [`crate::source::TrackSource`] - the playing one
//! by default, or the library selection - so a duplicate can watch each. A
//! click writes through the same catalog call the library table's rating
//! column makes, so a star set here shows everywhere else.

use gpui::{
    div, prelude::*, px, AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable,
    Pixels, SharedString, Subscription, WeakEntity, Window,
};
use gpui_component::menu::PopupMenu;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::TrackKey;
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::panel::{self, align_row, justify, Align, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::rating_ui;
use crate::selection::SelectionEvent;
use crate::settings::{rating_style, RatingStyle};
use crate::source::{self, ResolvedTrack, TrackSource};

/// The rating panel's per-view config: what a saved layout restores, and
/// what the settings window edits.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct RatingConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub source: TrackSource,
    #[serde(default)]
    pub align: Align,
}

/// The track the control currently sits over: the path it was resolved
/// from, that path's catalog id (None for a file the library does not
/// know), and the rating the id holds. Cached so the pump's per-frame
/// notifies never turn into database lookups.
struct Tracked {
    key: TrackKey,
    id: Option<i64>,
    rating: u8,
}

pub struct RatingPanel {
    state: AppState,
    config: RatingConfig,
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

impl RatingPanel {
    pub fn new(state: AppState, config: RatingConfig, cx: &mut Context<Self>) -> Self {
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
        // A star set anywhere else moves this one; a rescan can rewrite the
        // id -> path mapping under it, so that drops the whole cache.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| match event {
                LibraryEvent::Rated => this.refresh_rating(cx),
                LibraryEvent::Updated => {
                    this.resolved.invalidate();
                    this.track = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        RatingPanel {
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

    /// The shown track's id and rating, resolving and caching on a track
    /// change. Both None-ish while the source points at nothing, or at a
    /// file the library does not carry.
    fn current(&mut self, cx: &App) -> (Option<i64>, u8) {
        let Some(key) = self.resolved.get(self.config.source, &self.state, cx) else {
            self.track = None;
            return (None, 0);
        };
        if self.track.as_ref().map(|t| &t.key) != Some(&key) {
            let library = self.state.library.read(cx);
            let id = library.id_for_key(&key);
            let rating = id
                .and_then(|id| library.ratings_for(&[id]).get(&id).copied())
                .unwrap_or(0);
            self.track = Some(Tracked { key, id, rating });
        }
        self.track.as_ref().map_or((None, 0), |t| (t.id, t.rating))
    }

    /// Re-read the shown track's rating after a star landed, here or on
    /// another surface. The id stays put, so this costs a projection lookup
    /// rather than a resolve.
    fn refresh_rating(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.track.as_ref().and_then(|t| t.id) else {
            return;
        };
        let rating = self
            .state
            .library
            .read(cx)
            .ratings_for(&[id])
            .get(&id)
            .copied();
        if let (Some(track), Some(rating)) = (self.track.as_mut(), rating) {
            track.rating = rating;
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
        let (id, value) = self.current(cx);
        let state = self.state.clone();
        // Keyed by the shown track so the hover preview matches every
        // other surface rating the same track.
        let key = id.unwrap_or(-1) as u64;
        let control = rating_ui::control(key, value, move |rating, _, cx| {
            let Some(id) = id else { return };
            state
                .library
                .update(cx, |library, cx| library.rate(id, rating, cx));
        });
        div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .items_center()
            .map(|d| justify(d, self.config.align))
            .px(tokens::SPACE_MD)
            // Nothing to rate: the control stays up, dimmed, so the panel
            // holds its width in the strip instead of blinking out.
            .when(id.is_none(), |d| d.opacity(0.4))
            // The numeric strip is a scale, not a glyph run: give it the
            // room, where the stars sit wherever the alignment puts them.
            .child(control.when(rating_style() == RatingStyle::Numeric, |d| d.flex_1()))
    }
}

impl PanelSettings for RatingPanel {
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
        &[("Content", icons::STAR)]
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

impl Render for RatingPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

impl EventEmitter<PanelEvent> for RatingPanel {}

impl Focusable for RatingPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for RatingPanel {
    fn panel_name(&self) -> &'static str {
        "rating"
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Rating")
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
        // The five stars, or the numeric readout and its scale, plus the
        // strip's padding - raised by any user floor.
        let width = match rating_style() {
            RatingStyle::Stars => px(100.),
            RatingStyle::Numeric => px(140.),
        };
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(width, rox_dock::resizable::PANEL_MIN_SIZE),
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
                RatingPanel::new(state, config, cx)
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
