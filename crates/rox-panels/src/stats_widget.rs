//! The stats widget: the listen count over one trailing window for a
//! transport row, and the stats window's front door. The counts are the
//! stats page's indexed reads (ADR 11), cached rather than run per frame.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, App, Context, EventEmitter, FocusHandle, Focusable, SharedString, Subscription,
    WeakEntity, Window, div, prelude::*, px, svg,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Side};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings, setting_row, toggle};
use crate::panel_settings;
use crate::settings::ui as settings_ui;
use rox_services::history::HistoryEvent;

const DAY: i64 = 86400;

/// Re-count on a timer: the windows trail the clock, so an idle widget
/// would keep yesterday's "today". Nine indexed counts a minute cost nothing.
const TICK: Duration = Duration::from_secs(60);

/// Trailing, no calendar math: "today" is the last 24 hours.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListenRange {
    Day,
    #[default]
    Week,
    Month,
    Year,
    All,
}

impl ListenRange {
    fn span(self) -> Option<i64> {
        match self {
            ListenRange::Day => Some(DAY),
            ListenRange::Week => Some(7 * DAY),
            ListenRange::Month => Some(30 * DAY),
            ListenRange::Year => Some(365 * DAY),
            ListenRange::All => None,
        }
    }

    fn since(self, now: i64) -> i64 {
        self.span().map_or(0, |span| now - span)
    }

    /// The settings picker uses shorter labels so five segments fit its row.
    fn label(self) -> &'static str {
        match self {
            ListenRange::Day => rox_i18n::t_static("stats-range-today"),
            ListenRange::Week => rox_i18n::t_static("stats-range-week"),
            ListenRange::Month => rox_i18n::t_static("stats-range-month"),
            ListenRange::Year => rox_i18n::t_static("stats-range-year"),
            ListenRange::All => rox_i18n::t_static("stats-range-all"),
        }
    }
}

const ALL_RANGES: &[ListenRange] = &[
    ListenRange::Day,
    ListenRange::Week,
    ListenRange::Month,
    ListenRange::Year,
    ListenRange::All,
];

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StatsWidgetConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub range: ListenRange,
    pub show_count: bool,
    pub show_change: bool,
    pub open_on_click: bool,
}

impl Default for StatsWidgetConfig {
    fn default() -> Self {
        StatsWidgetConfig {
            chrome: PanelChrome::default(),
            range: ListenRange::default(),
            show_count: true,
            show_change: false,
            open_on_click: true,
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
struct Tally {
    count: u64,
    before: u64,
}

/// Every window counted at once, so neither the tooltip nor the chip
/// queries at hover.
#[derive(Default, PartialEq)]
struct Counts {
    day: Tally,
    week: Tally,
    month: Tally,
    year: Tally,
    total: u64,
}

impl Counts {
    fn get(&self, range: ListenRange) -> u64 {
        match range {
            ListenRange::Day => self.day.count,
            ListenRange::Week => self.week.count,
            ListenRange::Month => self.month.count,
            ListenRange::Year => self.year.count,
            ListenRange::All => self.total,
        }
    }

    fn change(&self, range: ListenRange) -> Option<i64> {
        let tally = match range {
            ListenRange::Day => self.day,
            ListenRange::Week => self.week,
            ListenRange::Month => self.month,
            ListenRange::Year => self.year,
            ListenRange::All => return None,
        };
        Some(tally.count as i64 - tally.before as i64)
    }
}

pub struct StatsWidgetPanel {
    state: AppState,
    config: StatsWidgetConfig,
    counts: Counts,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _history_changed: Subscription,
    /// A rescan drops tracks the events point at, and a play-count import
    /// writes listens directly.
    _library_changed: Subscription,
}

impl StatsWidgetPanel {
    pub fn new(state: AppState, config: StatsWidgetConfig, cx: &mut Context<Self>) -> Self {
        let _history_changed = cx.subscribe(
            &state.history,
            |this: &mut Self, _, _: &HistoryEvent, cx| this.refresh(cx),
        );
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Updated | LibraryEvent::PlaysReloaded) {
                    this.refresh(cx);
                }
            },
        );
        // The windows slide whether or not anything plays.
        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if view.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        let mut this = StatsWidgetPanel {
            state,
            config,
            counts: Counts::default(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            _history_changed,
            _library_changed,
        };
        this.refresh(cx);
        this
    }

    /// Repaint only when a number moved: the tick fires far more often than a
    /// listen.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let library = self.state.library.read(cx);
        // Two reads a window; the second minus the first is the window before it.
        let tally = |range: ListenRange| {
            let span = range.span().unwrap_or(0);
            let count = library.listens_since(range.since(now));
            Tally {
                count,
                before: library.listens_since(now - 2 * span).saturating_sub(count),
            }
        };
        let counts = Counts {
            day: tally(ListenRange::Day),
            week: tally(ListenRange::Week),
            month: tally(ListenRange::Month),
            year: tally(ListenRange::Year),
            total: library.listens_since(0),
        };
        if counts == self.counts {
            return;
        }
        self.counts = counts;
        cx.notify();
    }

    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu = menu
            .separator()
            .label(rox_i18n::t!("stats-readout-section"));
        // Live ticks through follow_panel, since the flyout stays open on a pick.
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            submenu = submenu.check_side(Side::Right);
            for range in ALL_RANGES.iter().copied() {
                submenu = submenu.item(panel::check_row(
                    range.label(),
                    None,
                    move |this: &Self| this.config.range == range,
                    move |this, cx| {
                        this.config.range = range;
                        cx.notify();
                    },
                    &panel,
                ));
            }
            submenu
        });
        let menu = menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("stats-count-menu"),
            submenu,
        ));
        // The booleans sit at the top level, where the menu closes on click and
        // a plain check is enough.
        let menu = self.toggle_item(
            menu,
            rox_i18n::t!("stats-show-number"),
            self.config.show_count,
            cx,
            |config| {
                config.show_count = !config.show_count;
            },
        );
        let menu = self.toggle_item(
            menu,
            rox_i18n::t!("stats-show-change"),
            self.config.show_change,
            cx,
            |config| config.show_change = !config.show_change,
        );
        self.toggle_item(
            menu,
            rox_i18n::t!("stats-click-opens"),
            self.config.open_on_click,
            cx,
            |config| config.open_on_click = !config.open_on_click,
        )
    }

    fn toggle_item(
        &self,
        menu: PopupMenu,
        label: impl Into<SharedString>,
        on: bool,
        cx: &mut Context<Self>,
        flip: impl Fn(&mut StatsWidgetConfig) + 'static,
    ) -> PopupMenu {
        let weak = cx.entity().downgrade();
        menu.item(
            PopupMenuItem::new(label)
                .checked(on)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        flip(&mut this.config);
                        cx.notify();
                    });
                }),
        )
    }

    fn rows(&self) -> Vec<TooltipRow> {
        ALL_RANGES
            .iter()
            .map(|range| TooltipRow {
                label: SharedString::from(range.label()),
                count: SharedString::from(rox_i18n::format::format_int(
                    self.counts.get(*range) as i64
                )),
                change: self
                    .config
                    .show_change
                    .then(|| self.counts.change(*range))
                    .flatten()
                    .map(|delta| SharedString::from(change_label(delta))),
                picked: *range == self.config.range,
            })
            .collect()
    }
}

/// Up takes the accent; a dip or a flat window stays muted rather than
/// reading as a fault.
fn change_look(delta: i64) -> (&'static str, gpui::Rgba) {
    match delta.signum() {
        1 => (icons::ARROW_UP, palette::accent()),
        -1 => (icons::ARROW_DOWN, palette::text_muted()),
        _ => (icons::MINUS, palette::text_faint()),
    }
}

fn change_label(delta: i64) -> String {
    if delta == 0 {
        "0".to_string()
    } else if delta > 0 {
        format!("+{}", rox_i18n::format::format_int(delta))
    } else {
        rox_i18n::format::format_int(delta)
    }
}

struct TooltipRow {
    label: SharedString,
    count: SharedString,
    change: Option<SharedString>,
    picked: bool,
}

/// Opaque fill, since it floats over panel content with no backdrop.
struct StatsTooltip {
    rows: Vec<TooltipRow>,
}

impl Render for StatsTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .p(tokens::SPACE_SM)
            .min_w(px(150.))
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
                    .child(rox_i18n::t!("stats-tooltip-listens")),
            )
            .children(self.rows.iter().map(|row| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(tokens::SPACE_MD)
                    .text_color(if row.picked {
                        palette::text_bright()
                    } else {
                        palette::text_secondary()
                    })
                    .child(div().min_w_0().truncate().child(row.label.clone()))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .flex_none()
                            .when_some(row.change.clone(), |d, change| {
                                d.child(div().text_color(palette::text_faint()).child(change))
                            })
                            .child(div().child(row.count.clone())),
                    )
            }))
    }
}

impl PanelSettings for StatsWidgetPanel {
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
        Some(
            div()
                .flex()
                .flex_col()
                .gap(settings_ui::SECTION_GAP)
                .child(settings_ui::section(
                    rox_i18n::t!("stats-readout-section"),
                    None,
                    div()
                        .flex()
                        .flex_col()
                        .gap(tokens::SPACE_MD)
                        .child(setting_row(
                            rox_i18n::t!("stats-count-menu"),
                            Some(rox_i18n::t!("stats-count-menu.description")),
                            panel::choices_shared(
                                &[
                                    (rox_i18n::t!("stats-range-day-short"), ListenRange::Day),
                                    (rox_i18n::t!("stats-range-week-short"), ListenRange::Week),
                                    (rox_i18n::t!("stats-range-month-short"), ListenRange::Month),
                                    (rox_i18n::t!("stats-range-year-short"), ListenRange::Year),
                                    (rox_i18n::t!("stats-range-all-short"), ListenRange::All),
                                ],
                                self.config.range,
                                |this: &mut Self, range, cx| {
                                    this.config.range = range;
                                    cx.notify();
                                },
                                cx,
                            ),
                        ))
                        .child(setting_row(
                            rox_i18n::t!("stats-show-number"),
                            Some(rox_i18n::t!("stats-show-number.description")),
                            toggle(
                                self.config.show_count,
                                |this: &mut Self, on, cx| {
                                    this.config.show_count = on;
                                    cx.notify();
                                },
                                cx,
                            ),
                        ))
                        .child(setting_row(
                            rox_i18n::t!("stats-show-change"),
                            Some(rox_i18n::t!("stats-show-change.description")),
                            toggle(
                                self.config.show_change,
                                |this: &mut Self, on, cx| {
                                    this.config.show_change = on;
                                    cx.notify();
                                },
                                cx,
                            ),
                        )),
                ))
                .child(settings_ui::section(
                    rox_i18n::t!("stats-click-section"),
                    None,
                    setting_row(
                        rox_i18n::t!("stats-open-on-click"),
                        Some(rox_i18n::t!("stats-open-on-click.description")),
                        toggle(
                            self.config.open_on_click,
                            |this: &mut Self, on, cx| {
                                this.config.open_on_click = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    ),
                ))
                .into_any_element(),
        )
    }
}

impl EventEmitter<PanelEvent> for StatsWidgetPanel {}

impl Focusable for StatsWidgetPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for StatsWidgetPanel {
    fn panel_name(&self) -> &'static str {
        "stats widget"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("stats-title"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        // Room for a four-digit count, and for the chip.
        let mut width = rox_dock::resizable::PANEL_MIN_SIZE;
        if self.config.show_count {
            width += px(24.);
        }
        if self.config.show_change {
            width += px(30.);
        }
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(width, rox_dock::resizable::PANEL_MIN_SIZE),
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
        let state = self.state.clone();
        let menu = menu.item(
            PopupMenuItem::new(rox_i18n::t!("stats-open"))
                .icon(Icon::default().path(icons::CHART_PIE))
                .on_click(move |_, _, cx| {
                    rox_panel_api::openers::stats_window(state.clone(), cx);
                }),
        );
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
                StatsWidgetPanel::new(state, config, cx)
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

impl Render for StatsWidgetPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        let count = self.counts.get(self.config.range);
        let show_count = self.config.show_count;
        let change = self
            .config
            .show_change
            .then(|| self.counts.change(self.config.range))
            .flatten();
        let open_on_click = self.config.open_on_click;
        let weak = cx.entity().downgrade();
        let focus = self.focus.clone();
        panel::themed(&chrome, move || {
            div()
                .size_full()
                .bg(palette::bg_root())
                .track_focus(&focus)
                .child(
                    div()
                        .id("stats-widget")
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .gap(tokens::SPACE_XS)
                        .px(tokens::SPACE_SM)
                        .size_full()
                        .when(open_on_click, |d| {
                            let weak = weak.clone();
                            d.cursor_pointer().on_click(move |_, _, cx| {
                                if let Some(state) =
                                    weak.upgrade().map(|this| this.read(cx).state.clone())
                                {
                                    rox_panel_api::openers::stats_window(state, cx);
                                }
                            })
                        })
                        .child(
                            svg()
                                .path(icons::CHART_PIE)
                                .size(px(16.))
                                .flex_none()
                                .text_color(if count > 0 {
                                    palette::text()
                                } else {
                                    palette::text_muted()
                                }),
                        )
                        .when(show_count, |d| {
                            d.child(
                                div()
                                    .whitespace_nowrap()
                                    .text_xs()
                                    .text_color(if count > 0 {
                                        palette::text()
                                    } else {
                                        palette::text_muted()
                                    })
                                    .child(SharedString::from(rox_i18n::format::format_int(
                                        count as i64,
                                    ))),
                            )
                        })
                        .when_some(change, |d, delta| {
                            let (icon, color) = change_look(delta);
                            d.child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .flex_none()
                                    .whitespace_nowrap()
                                    .text_xs()
                                    .text_color(color)
                                    .child(
                                        svg()
                                            .path(icon)
                                            .size(px(11.))
                                            .flex_none()
                                            .text_color(color),
                                    )
                                    // The dash covers a flat window, so no zero beside it.
                                    .when(delta != 0, |d| {
                                        d.child(SharedString::from(rox_i18n::format::format_int(
                                            delta.unsigned_abs() as i64,
                                        )))
                                    }),
                            )
                        })
                        .tooltip(move |_window, cx| {
                            let rows = weak
                                .upgrade()
                                .map(|this| this.read(cx).rows())
                                .unwrap_or_default();
                            cx.new(|_| StatsTooltip { rows }).into()
                        }),
                )
        })
    }
}
