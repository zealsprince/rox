//! The queue widget (ADR 16): a compact queue icon with a badge counting the
//! explicit up-next tracks, and a hover tooltip listing the next few. For a
//! transport row, where the full queue panel would be too much. Reads the same
//! explicit queue as the queue panel, so the context (the album or library
//! playing on) stays off the count.

use gpui::{
    div, prelude::*, px, svg, App, Context, EventEmitter, FocusHandle, Focusable, SharedString,
    Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::Icon;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;

/// How many titles the hover tooltip lists before summarizing the rest.
const TOOLTIP_ROWS: usize = 12;

/// The widget's config: just the shared chrome, no knobs of its own.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct QueueWidgetConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
}

pub struct QueueWidgetPanel {
    state: AppState,
    config: QueueWidgetConfig,
    /// The explicit queue count, the badge number. Cached so the per-pump
    /// observe repaints only when it changes.
    count: usize,
    /// The cheap change detector: the queue revision and the playing path, the
    /// two things that move the count.
    rev: Option<u64>,
    playing_path: Option<std::path::PathBuf>,
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
            playing_path: None,
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
        };
        this.sync(cx);
        this
    }

    /// Refresh the badge count, bailing on the cheap revision and playing-path
    /// compare so a steady queue costs two reads per tick.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let rev = self.state.player.read(cx).queue_rev();
        let playing_path = self.state.player.read(cx).now_playing().map(|now| now.path);
        if rev == self.rev && playing_path == self.playing_path {
            return;
        }
        self.rev = rev;
        self.playing_path = playing_path;
        self.count = self.state.player.read(cx).queued_count();
        cx.notify();
    }

    /// The tooltip's rows: the next titles with their artists, resolved
    /// fresh at hover.
    fn next_up(&self, cx: &App) -> Vec<(SharedString, SharedString)> {
        let queued = self.state.player.read(cx).queued();
        let library = self.state.library.read(cx);
        queued
            .iter()
            .take(TOOLTIP_ROWS)
            .map(|entry| {
                let meta = library.meta_for(&entry.path);
                let title = meta
                    .as_ref()
                    .map(|meta| meta.title.clone())
                    .filter(|title| !title.is_empty())
                    .or_else(|| {
                        entry
                            .path
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

/// The hover tooltip: a small "up next" list of the queued titles. Reads
/// its fill opaque like the popup menus - it floats over panel content
/// with no backdrop behind it, so surface opacity stays off.
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
            .child(div().text_color(palette::text_muted()).child("Up Next"))
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
                        .child(SharedString::from(format!("+{} more", self.more))),
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

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Queue Widget")
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
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
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let state = self.state.clone();
        let menu = menu.item(
            PopupMenuItem::new("Clear Queue")
                .icon(Icon::default().path(icons::TRASH))
                .disabled(self.count == 0)
                .on_click(move |_, _, cx| {
                    state.player.read(cx).clear_queue();
                }),
        );
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), _window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
        )
    }
}

impl Render for QueueWidgetPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        let count = self.count;
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
                            // The badge: a small accent pill floating off the
                            // icon's corner, so the widget's footprint never
                            // shifts with the count. Anchored by its left edge;
                            // a wider count grows away from the icon.
                            .when(count > 0, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top(px(-6.))
                                        .left(px(10.))
                                        .px(px(4.))
                                        // The parent is the 16px icon, so the
                                        // count has to refuse that width or a
                                        // two-digit badge wraps into a stack.
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
                    // The hover list of the next titles.
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
