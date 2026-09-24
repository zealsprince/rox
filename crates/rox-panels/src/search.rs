//! The search panel: a dockable box that drives the shared app-wide query
//! ([`crate::query::shared_query`]). The query lives in the shared entity
//! rather than this panel's config, so every search panel edits and shows
//! the same value.

use gpui::{
    App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Subscription,
    WeakEntity, Window, div, prelude::*,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::query::search::{SearchBox, SearchEvent};
use crate::query::shared_query::SharedQueryEvent;
use rox_panel_api::suggest;

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChipsPlacement {
    #[default]
    Inline,
    Below,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SearchConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub chips: ChipsPlacement,
}

pub struct SearchPanel {
    state: AppState,
    config: SearchConfig,
    search: Entity<SearchBox>,
    /// The escape ladder's target: a bare escape hands the playback keys back
    /// to the workspace.
    focus: FocusHandle,
    /// Applied on the next render, where a window exists to set the input.
    resync_box: bool,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _search_events: Subscription,
    _query_changed: Subscription,
    _library_changed: Subscription,
    /// Drops the panel's entry from the shared query's box count on release.
    _query_boxes: Subscription,
}

impl SearchPanel {
    pub fn new(
        state: AppState,
        config: SearchConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial = state.query.read(cx).text().to_string();
        // Bare and one line tall: the panel frames the box itself.
        let search = cx.new(|cx| {
            SearchBox::new(rox_i18n::t!("search-placeholder"), &initial, window, cx)
                .bare()
                .xsmall()
                .icon()
        });
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        // Mirror another box's edits. The reset needs a window, so it waits for
        // the next render.
        let _query_changed = cx.subscribe(
            &state.query,
            |this: &mut Self, _, _: &SharedQueryEvent, cx| {
                this.resync_box = true;
                cx.notify();
                panel::refresh_tab_panel(&this.tab_panel, cx);
            },
        );
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Updated) {
                    this.attach_suggestions(cx);
                }
            },
        );
        // Counted so a follower's jump-to knows a box exists to show it.
        state.query.update(cx, |q, _| q.register_box());
        let query = state.query.clone();
        let _query_boxes = cx.on_release(move |_, cx| {
            query.update(cx, |q, _| q.release_box());
        });
        let this = SearchPanel {
            state,
            config,
            search,
            focus: cx.focus_handle(),
            resync_box: false,
            tab_panel: None,
            _search_events,
            _query_changed,
            _library_changed,
            _query_boxes,
        };
        this.attach_suggestions(cx);
        this
    }

    fn attach_suggestions(&self, cx: &mut Context<Self>) {
        let provider = suggest::query_provider(&self.state.library, cx);
        self.search
            .update(cx, |search, cx| search.set_completions(provider, cx));
    }

    /// Guarded on drift, so the box being typed in keeps its cursor and
    /// doesn't echo.
    fn sync_box(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.state.query.read(cx).text().to_string();
        self.search.update(cx, |search, cx| {
            if search.query() != text {
                search.set_value(&text, window, cx);
            }
        });
    }

    fn on_search_event(
        &mut self,
        search: &Entity<SearchBox>,
        event: &SearchEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SearchEvent::Changed => {
                let text = search.read(cx).query().to_string();
                self.state.query.update(cx, |q, cx| q.set(text, cx));
                cx.notify();
                panel::refresh_tab_panel(&self.tab_panel, cx);
            }
            SearchEvent::FocusChanged => {
                cx.notify();
                panel::refresh_tab_panel(&self.tab_panel, cx);
            }
            SearchEvent::Dismissed => {
                window.focus(&self.focus);
                cx.notify();
                panel::refresh_tab_panel(&self.tab_panel, cx);
            }
            SearchEvent::Submitted => {}
        }
    }

    fn chips_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            for (label, place) in [
                (rox_i18n::t!("search-chips-inline"), ChipsPlacement::Inline),
                (rox_i18n::t!("search-chips-below"), ChipsPlacement::Below),
            ] {
                submenu = submenu.item(panel::check_row(
                    label,
                    None,
                    move |this: &Self| this.config.chips == place,
                    move |this, cx| {
                        this.config.chips = place;
                        cx.notify();
                    },
                    &panel,
                ));
            }
            submenu
        });
        menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("search-filter-chips"),
            submenu,
        ))
    }
}

impl EventEmitter<PanelEvent> for SearchPanel {}

impl Focusable for SearchPanel {
    /// Activating the tab focuses the box.
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.search.read(cx).focus_handle(cx)
    }
}

impl PanelSettings for SearchPanel {
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

impl Panel for SearchPanel {
    fn panel_name(&self) -> &'static str {
        "search"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("query-search"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    /// Shrinks to about a tab's height instead of the 40px floor; the width
    /// keeps the floor.
    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                tokens::CONTROL_H + tokens::SPACE_XS + tokens::SPACE_XS,
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
        let menu = self.chips_menu(menu, window, cx);
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
                SearchPanel::new(state, config, window, cx)
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

impl Render for SearchPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl SearchPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        if self.resync_box {
            self.resync_box = false;
            self.sync_box(window, cx);
        }
        let input = self.search.update(cx, |search, cx| search.element(cx));
        let chips = crate::query::shared_query::filter_chips(&self.state.query, cx);
        let base = div()
            .track_focus(&self.focus)
            .size_full()
            .bg(palette::bg_root())
            .px(tokens::SPACE_SM);
        match self.config.chips {
            ChipsPlacement::Inline => base
                .flex()
                .items_center()
                .gap(tokens::SPACE_SM)
                .child(input.flex_1())
                .when_some(chips, |d, chips| d.child(chips.flex_none())),
            ChipsPlacement::Below => base
                .flex()
                .flex_col()
                .justify_center()
                .py(tokens::SPACE_XS)
                .gap(tokens::SPACE_XS)
                .child(input.w_full())
                .when_some(chips, |d, chips| d.child(chips)),
        }
    }
}
