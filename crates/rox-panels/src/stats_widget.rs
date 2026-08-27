//! The stats widget: the listening record boiled down to one number for
//! a transport row, and the stats window's front door. Counts the listens
//! inside one trailing window, with the other windows on hover, so the
//! record is a glance away without giving up a panel-sized surface. The
//! counts are the stats page's own indexed reads (ADR 11), run when a
//! listen is recorded rather than per frame.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    div, prelude::*, px, svg, AnyElement, App, Context, EventEmitter, FocusHandle, Focusable,
    SharedString, Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Side};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::panel::{self, setting_row, toggle, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::settings::ui as settings_ui;
use rox_services::history::HistoryEvent;

const DAY: i64 = 86400;

/// How often the counts re-run with nothing else prompting them. The
/// windows trail the clock, so an idle widget would show yesterday's
/// "today" until the next listen arrived; nine indexed counts a minute
/// costs nothing next to showing a stale number for hours.
const TICK: Duration = Duration::from_secs(60);

/// Which trailing window the readout counts over. Trailing, no calendar
/// math, same as the stats page's rows: "today" is the last 24 hours.
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
    /// How wide the window is in seconds. None for All Time, which has
    /// no width and so nothing behind it for the change chip.
    fn span(self) -> Option<i64> {
        match self {
            ListenRange::Day => Some(DAY),
            ListenRange::Week => Some(7 * DAY),
            ListenRange::Month => Some(30 * DAY),
            ListenRange::Year => Some(365 * DAY),
            ListenRange::All => None,
        }
    }

    /// The window's lower bound in unix seconds; 0 counts every event.
    fn since(self, now: i64) -> i64 {
        self.span().map_or(0, |span| now - span)
    }

    /// The window spelled out, for the tooltip and the readout's own
    /// hover copy. The settings picker uses shorter labels so five
    /// segments still fit its row.
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

/// Every window in order, the tooltip's rows and the menu flyout's; the
/// readout's own range reads brighter among the tooltip's.
const ALL_RANGES: &[ListenRange] = &[
    ListenRange::Day,
    ListenRange::Week,
    ListenRange::Month,
    ListenRange::Year,
    ListenRange::All,
];

/// The widget's config: the shared chrome plus what it counts, whether
/// the count shows at all, and whether a click opens the stats window.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StatsWidgetConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// Which trailing window the readout counts over.
    pub range: ListenRange,
    /// Draw the count beside the icon. Off leaves a bare icon, for a
    /// strip that only needs the way in to the stats window.
    pub show_count: bool,
    /// Draw the change chip: how this window compares with the window
    /// right before it, up or down. All Time has nothing behind it, so
    /// the chip is hidden on that range.
    pub show_change: bool,
    /// Click the widget to open the stats window. On by default; off
    /// leaves it a readout.
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

/// One window's pair of numbers: the listens inside it, and the listens
/// in the window right before it, which the change chip subtracts.
#[derive(Clone, Copy, Default, PartialEq)]
struct Tally {
    count: u64,
    before: u64,
}

/// Every window's listen count, measured together. Two COUNTs over the
/// played_at index per window, so taking them all costs about what
/// taking the picked one would, and neither the tooltip nor the chip
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

    /// How this window compares with the one before it, positive for up.
    /// None for All Time, which has no window behind it.
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
    /// The cached counts, so a repaint never touches the database.
    counts: Counts,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// A new listen moves every number here.
    _history_changed: Subscription,
    /// A rescan can drop tracks the events point at, which moves the
    /// rollups the stats window shows beside these counts.
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
                if matches!(event, LibraryEvent::Updated) {
                    this.refresh(cx);
                }
            },
        );
        // The trailing windows slide whether or not anything plays, so
        // re-count on a slow tick; the loop ends with the view, the
        // console window's shape.
        cx.spawn(async move |view, cx| loop {
            cx.background_executor().timer(TICK).await;
            if view.update(cx, |this, cx| this.refresh(cx)).is_err() {
                break;
            }
        })
        .detach();
        let mut this = StatsWidgetPanel {
            state,
            config,
            counts: Counts::default(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _history_changed,
            _library_changed,
        };
        this.refresh(cx);
        this
    }

    /// Re-count every window, repainting only when a number actually
    /// moved: the minute tick fires far more often than a listen does.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let library = self.state.library.read(cx);
        // Two reads a window: everything since it opened, and everything
        // since the window before it opened. Subtract the first from the
        // second and what's left is the earlier window on its own.
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

    /// The panel's own quick entries: the range flyout and the readout
    /// toggles, so the widget can be re-aimed from its right-click
    /// without a trip through the settings window.
    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu = menu
            .separator()
            .label(rox_i18n::t!("stats-readout-section"));
        // The range as a flyout, with live ticks through follow_panel +
        // check_row: the flyout stays open on a pick, so a tick baked in
        // at build time would stay on the old range.
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
        // The three booleans are at the top level, where the menu closes
        // on the click and a plain check shows the state fine.
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

    /// One checked menu row over a config boolean.
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

    /// The tooltip's rows: every window's count with the picked one
    /// marked, read off the cache. The change is included only when the
    /// chip is on, so the tooltip stays a plain list otherwise.
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

/// The chip's three states, picked off the sign: which way the arrow
/// points and how loud it reads. Up takes the accent because a climbing
/// record is the thing worth catching from across the strip; a dip and a
/// flat window step back rather than reading as a fault, the way the
/// status tones would.
fn change_look(delta: i64) -> (&'static str, gpui::Rgba) {
    match delta.signum() {
        1 => (icons::ARROW_UP, palette::accent()),
        -1 => (icons::ARROW_DOWN, palette::text_muted()),
        _ => (icons::MINUS, palette::text_faint()),
    }
}

/// The change spelled out with its sign, the tooltip's column.
fn change_label(delta: i64) -> String {
    if delta == 0 {
        "0".to_string()
    } else if delta > 0 {
        format!("+{}", rox_i18n::format::format_int(delta))
    } else {
        rox_i18n::format::format_int(delta)
    }
}

/// One window's line in the tooltip.
struct TooltipRow {
    label: SharedString,
    count: SharedString,
    /// The signed change against the window before, when the chip is on.
    change: Option<SharedString>,
    /// The readout's own range, which reads brighter than the rest.
    picked: bool,
}

/// The hover tooltip: the same counts over every window, so the picked
/// one has something to compare against. Opaque fill like the popup menus,
/// since it floats over panel content with no backdrop behind it.
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
        // The count runs to four digits on an old record, so the floor
        // widens with it, and again with the chip's arrow and delta; the
        // bare icon keeps the strip's own minimum.
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
        panel::themed(&chrome, move || {
            div().size_full().bg(palette::bg_root()).child(
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
                                // The strip is short, and a four-digit
                                // count would otherwise wrap.
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
                                .child(svg().path(icon).size(px(11.)).flex_none().text_color(color))
                                // The dash already covers a flat window;
                                // a zero beside it would just be another
                                // digit to read past.
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
