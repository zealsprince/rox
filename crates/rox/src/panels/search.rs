//! The search panel: a dockable box that drives the shared app-wide query
//! ([`crate::shared_query`]). Its whole job is the box - typing here filters
//! every panel set to follow the shared query, so a library and a couple of
//! grids can sit query-less and clean while this one controls them all. The
//! query lives in the shared entity, not this panel's config, so two search
//! panels and a popped-out one all edit and mirror the same value. Suggestions
//! come from the projection's tag values, reattached on each scan the way the
//! play launcher does.

use std::sync::Arc;

use gpui::{
    div, prelude::*, App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, SharedString,
    Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::tokens;
use crate::panel::{self, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::panels::library::LibraryEvent;
use crate::search::{SearchBox, SearchEvent};
use crate::shared_query::SharedQueryEvent;
use crate::suggest;

/// Where the search panel shows the shared filter's chips: inline, trailing
/// the box on the same line, or on their own row below it.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChipsPlacement {
    #[default]
    Inline,
    Below,
}

/// The search panel's per-view config. The query is not here - it lives in
/// the shared entity - so a saved layout only restores the rename, the
/// panel's look, and where the filter chips sit.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SearchConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// Where the active-filter chips sit relative to the box.
    #[serde(default)]
    pub chips: ChipsPlacement,
}

pub struct SearchPanel {
    state: AppState,
    config: SearchConfig,
    /// The query editor bound to the shared query: it writes on change and
    /// mirrors the shared value in.
    search: Entity<SearchBox>,
    /// The panel's own focus, the escape ladder's target so a bare escape
    /// hands the playback keys back to the workspace.
    focus: FocusHandle,
    /// A pending box reset from a shared-query change; applied on the next
    /// render, where a window exists to set the input's text.
    resync_box: bool,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
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
        // Bare and a single font line tall: the panel frames the box itself,
        // so drop the input's border and rounding and let it collapse to a
        // thin bar.
        let search = cx.new(|cx| {
            SearchBox::new("Search the library", &initial, window, cx)
                .bare()
                .xsmall()
                .icon()
        });
        let _search_events = cx.subscribe_in(&search, window, Self::on_search_event);
        // Mirror the shared query in when another box changes it, so two
        // search panels and a popped-out one stay in sync. The reset needs a
        // window, so it rides the resync flag.
        let _query_changed = cx.subscribe(
            &state.query,
            |this: &mut Self, _, _: &SharedQueryEvent, cx| {
                this.resync_box = true;
                cx.notify();
                panel::refresh_tab_panel(&this.tab_panel, cx);
            },
        );
        // A scan lands a new projection; point the suggestions at it.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Updated) {
                    this.attach_suggestions(cx);
                }
            },
        );
        // Count this panel while it lives so a jump-to from a follower knows
        // the shared query has a box to show it, and stop counting on release.
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

    /// Point the box's suggestion menu at the current projection; at open and
    /// again whenever a scan lands a new one.
    fn attach_suggestions(&self, cx: &mut Context<Self>) {
        let provider = {
            let library = self.state.library.read(cx);
            suggest::query_provider(library.projection())
        };
        self.search
            .update(cx, |search, cx| search.set_completions(provider, cx));
    }

    /// Reset the box to the shared query, cursor to the end. Guarded on drift
    /// so the box the user is typing in keeps its cursor, which also stops the
    /// mirror echo.
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
            // Publish to the shared query; the followers and any other search
            // box rebuild off the shared-query subscription.
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
            // Escape on an empty query leaves the box, handing the playback
            // keys back to the workspace.
            SearchEvent::Dismissed => {
                window.focus(&self.focus);
                cx.notify();
                panel::refresh_tab_panel(&self.tab_panel, cx);
            }
            SearchEvent::Submitted => {}
        }
    }

    /// The Filter Chips submenu: where the active-filter chips sit relative
    /// to the box, inline or below.
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
                ("Inline", ChipsPlacement::Inline),
                ("Below", ChipsPlacement::Below),
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
        menu.item(PopupMenuItem::submenu("Filter Chips", submenu))
    }
}

impl EventEmitter<PanelEvent> for SearchPanel {}

impl Focusable for SearchPanel {
    /// Activating the tab focuses the box, since typing is the whole point.
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

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Search")
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    /// The box is one line tall, so let the panel shrink to about a tab's
    /// height instead of holding the global 40px floor: the xsmall control
    /// plus a hair of air top and bottom. Width keeps the global floor.
    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        gpui::size(
            rox_dock::resizable::PANEL_MIN_SIZE,
            tokens::CONTROL_H + tokens::SPACE_XS + tokens::SPACE_XS,
        )
    }

    /// The layout dump carries the panel's config; the builder registered in
    /// `workspace::register_panels` reads it back.
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
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        // Duplicate hand-rolled rather than through `panel::duplicate_item`
        // because the copy takes the config along, like the cover panel's; the
        // two boxes then drive and mirror the one shared query.
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Duplicate")
                .icon(Icon::default().path(icons::COPY))
                .on_click(move |_, window, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    let (state, config, tabs) = {
                        let panel = this.read(cx);
                        (
                            panel.state.clone(),
                            panel.config.clone(),
                            panel.tab_panel.clone(),
                        )
                    };
                    let Some(tabs) = tabs.and_then(|tabs| tabs.upgrade()) else {
                        return;
                    };
                    let dup = cx.new(|cx| SearchPanel::new(state, config, window, cx));
                    tabs.update(cx, |tabs, cx| tabs.add_panel(Arc::new(dup), window, cx));
                }),
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
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
        // A pending box reset from a shared-query change lands here, where a
        // window exists to set the input's text.
        if self.resync_box {
            self.resync_box = false;
            self.sync_box(window, cx);
        }
        let input = self.search.update(cx, |search, cx| search.element(cx));
        // The active-filter chips, shown only when something is filtered.
        let chips = crate::shared_query::filter_chips(&self.state.query, cx);
        let base = div()
            .track_focus(&self.focus)
            .size_full()
            .px(tokens::SPACE_SM);
        match self.config.chips {
            // Inline: the box takes the line, the chips trail it, so the
            // magnifier keeps the left and the bar stays one row.
            ChipsPlacement::Inline => base
                .flex()
                .items_center()
                .gap(tokens::SPACE_SM)
                .child(input.flex_1())
                .when_some(chips, |d, chips| d.child(chips.flex_none())),
            // Below: the box on top, the chips wrapping on their own row
            // under it. Centered so a taller slot splits the air evenly.
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
