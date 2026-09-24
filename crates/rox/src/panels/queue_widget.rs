//! The queue widget (ADR 16): a queue icon with a badge counting the explicit
//! up-next tracks and a tooltip listing the next few. The context playing on
//! stays off the count.

use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, EventEmitter, FocusHandle, Focusable, SharedString, Subscription,
    WeakEntity, Window, div, prelude::*, px, svg,
};
use gpui_component::Icon;
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::TrackKey;
use serde::{Deserialize, Serialize};

use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel::{self, AppState, PanelChrome, PanelSettings};
use rox_panel_api::panel_settings;
use rox_panel_kit::ui as settings_ui;
use rox_panel_kit::{setting_row, toggle};
use rox_panels::queue::QueuePanel;

const TOOLTIP_ROWS: usize = 12;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QueueWidgetConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub open_on_click: bool,
    pub always_modal: bool,
}

impl Default for QueueWidgetConfig {
    fn default() -> Self {
        QueueWidgetConfig {
            chrome: PanelChrome::default(),
            open_on_click: true,
            always_modal: false,
        }
    }
}

pub struct QueueWidgetPanel {
    state: AppState,
    config: QueueWidgetConfig,
    /// Cached so the per-pump observe repaints only when it changes.
    count: usize,
    rev: Option<u64>,
    playing_key: Option<TrackKey>,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
}

impl QueueWidgetPanel {
    pub fn new(state: AppState, config: QueueWidgetConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |this: &mut Self, _, cx| this.sync(cx));
        let mut this = QueueWidgetPanel {
            state,
            config,
            count: 0,
            rev: None,
            playing_key: None,
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
        };
        this.sync(cx);
        this
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let rev = self.state.player.read(cx).queue_rev();
        let playing_key = self.state.player.read(cx).now_playing().map(|now| now.key);
        if rev == self.rev && playing_key == self.playing_key {
            return;
        }
        self.rev = rev;
        self.playing_key = playing_key;
        self.count = self.state.player.read(cx).queued_count();
        cx.notify();
    }

    /// Falls back to a queue window when popped out with no workspace behind
    /// it.
    ///
    /// Takes the state rather than `&self`: `focus_panel_named` reads every
    /// docked panel, this one included, and a read inside its own update
    /// panics.
    fn open_queue(state: &AppState, always_modal: bool, window: &mut Window, cx: &mut App) {
        if !always_modal && panel::focus_panel_named(&state.tab_hosts, "queue", window, cx) {
            return;
        }
        if let Some(workspace) =
            crate::workspace::workspace_for_window(window, cx).and_then(|ws| ws.upgrade())
        {
            workspace.update(cx, |ws, cx| ws.toggle_queue_modal(window, cx));
            return;
        }
        let queue = cx.new(|cx| QueuePanel::windowed(state.clone(), window, cx));
        panel::open_panel_window(Arc::new(queue), state.clone(), cx);
    }

    fn next_up(&self, cx: &App) -> Vec<(SharedString, SharedString)> {
        let player = self.state.player.read(cx);
        let queued = player.queued();
        let library = self.state.library.read(cx);
        queued
            .iter()
            .take(TOOLTIP_ROWS)
            .map(|entry| {
                // Through the pool mirror, so two cue tracks of one image list
                // as themselves.
                let key = player.key_for(entry);
                let meta = library.meta_for_key(&key);
                let title = meta
                    .as_ref()
                    .map(|meta| meta.title.clone())
                    .filter(|title| !title.is_empty())
                    .or_else(|| {
                        key.path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                    })
                    .unwrap_or_default();
                let artist = meta.map(|meta| meta.artist).unwrap_or_default();
                (SharedString::from(title), SharedString::from(artist))
            })
            .collect()
    }
}

/// Opaque fill: it floats over panel content with no backdrop behind it.
struct QueueTooltip {
    rows: Vec<(SharedString, SharedString)>,
    more: usize,
}

impl Render for QueueTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .p(tokens::SPACE_SM)
            .max_w(px(280.))
            .rounded(tokens::RADIUS)
            .border_1()
            .border_color(palette::border())
            .bg(palette::bg_menu_opaque())
            .shadow_md()
            .text_color(palette::text())
            .text_xs()
            .child(
                div()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("queue-widget-up-next")),
            )
            .children(self.rows.iter().map(|(title, artist)| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(div().flex_1().min_w_0().truncate().child(title.clone()))
                    .when(!artist.is_empty(), |d| {
                        d.child(
                            div()
                                .flex_none()
                                .max_w(px(110.))
                                .truncate()
                                .text_color(palette::text_muted())
                                .child(artist.clone()),
                        )
                    })
            }))
            .when(self.more > 0, |d| {
                d.child(
                    div()
                        .text_color(palette::text_muted())
                        .child(rox_i18n::t!("queue-widget-more", count = self.more as u64)),
                )
            })
    }
}

impl PanelSettings for QueueWidgetPanel {
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

    fn behavior(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mut rows = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(setting_row(
                rox_i18n::t!("queue-widget-open-on-click"),
                Some(rox_i18n::t!("queue-widget-open-on-click.description")),
                toggle(
                    self.config.open_on_click,
                    |this: &mut Self, on, cx| {
                        this.config.open_on_click = on;
                        cx.notify();
                    },
                    cx,
                ),
            ));
        if self.config.open_on_click {
            rows = rows.child(setting_row(
                rox_i18n::t!("queue-widget-always-modal"),
                Some(rox_i18n::t!("queue-widget-always-modal.description")),
                toggle(
                    self.config.always_modal,
                    |this: &mut Self, on, cx| {
                        this.config.always_modal = on;
                        cx.notify();
                    },
                    cx,
                ),
            ));
        }
        Some(
            settings_ui::section(rox_i18n::t!("queue-widget-section-click"), None, rows)
                .into_any_element(),
        )
    }
}

impl EventEmitter<PanelEvent> for QueueWidgetPanel {}

impl Focusable for QueueWidgetPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for QueueWidgetPanel {
    fn panel_name(&self) -> &'static str {
        "queue widget"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("queue-widget-title"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        rox_panel_api::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
        )
    }

    fn max_size(&self, cx: &App) -> gpui::Size<gpui::Pixels> {
        rox_panel_api::panel::chrome_max_size(&self.config.chrome, self.min_size(cx))
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
        let state = self.state.clone();
        let menu = menu.item(
            PopupMenuItem::new(rox_i18n::t!("queue-widget-clear-queue"))
                .icon(Icon::default().path(icons::TRASH))
                .disabled(self.count == 0)
                .on_click(move |_, _, cx| {
                    state.player.read(cx).clear_queue();
                }),
        );
        let menu =
            panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity(), cx);
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
            window,
        )
    }
}

impl Render for QueueWidgetPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        let count = self.count;
        let open_on_click = self.config.open_on_click;
        let weak = cx.entity().downgrade();
        panel::themed(&chrome, move || {
            div().size_full().bg(palette::bg_root()).child(
                div()
                    .id("queue-widget")
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .px(tokens::SPACE_SM)
                    .size_full()
                    .when(open_on_click, |d| {
                        let weak = weak.clone();
                        d.cursor_pointer().on_click(move |_, window, cx| {
                            if let Some(this) = weak.upgrade() {
                                let (state, always_modal) = {
                                    let this = this.read(cx);
                                    (this.state.clone(), this.config.always_modal)
                                };
                                Self::open_queue(&state, always_modal, window, cx);
                            }
                        })
                    })
                    .child(
                        div()
                            .relative()
                            .child(svg().path(icons::LIST_MUSIC).size(px(16.)).text_color(
                                if count > 0 {
                                    palette::text()
                                } else {
                                    palette::text_muted()
                                },
                            ))
                            // Floats off the icon's corner so the footprint never
                            // shifts with the count.
                            .when(count > 0, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(px(-6.))
                                        .left(px(10.))
                                        .px(px(4.))
                                        // One line, or a two-digit badge wraps inside the 16px
                                        // parent.
                                        .whitespace_nowrap()
                                        .rounded_full()
                                        .bg(palette::accent())
                                        .text_color(palette::text_on_accent())
                                        .text_size(px(9.))
                                        .line_height(px(12.))
                                        .child(SharedString::from(count.to_string())),
                                )
                            }),
                    )
                    .when(count > 0, |d| {
                        let weak = weak.clone();
                        d.tooltip(move |_window, cx| {
                            let (rows, more) = weak
                                .upgrade()
                                .map(|this| {
                                    let this = this.read(cx);
                                    (this.next_up(cx), this.count.saturating_sub(TOOLTIP_ROWS))
                                })
                                .unwrap_or_default();
                            cx.new(|_| QueueTooltip { rows, more }).into()
                        })
                    }),
            )
        })
    }
}
