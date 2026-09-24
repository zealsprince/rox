//! The health widget: the library's tag coverage as one percentage for a
//! transport row, broken out per tag on hover, and the health window's front
//! door. The number is [`rox_library::health::completeness`]'s, the same walk
//! the health window reads, recomputed when the catalog changes.
//!
//! Never measures anything that costs a file read. Art, duplicates, and album
//! gaps belong to the health window's background pass.

use std::time::Duration;

use gpui::{
    AnyElement, App, Context, EventEmitter, FocusHandle, Focusable, SharedString, Subscription,
    Task, WeakEntity, Window, div, prelude::*, px, svg,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Side};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::health::{self, Check, Completeness};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings, setting_row, toggle};
use crate::panel_settings;
use crate::settings::ui as settings_ui;

/// How long the widget waits before re-walking. A reload raises
/// `LibraryEvent::Updated` twice and a scan once per batch, so one edit
/// arrives as a burst.
const SCAN_DEBOUNCE: Duration = Duration::from_millis(200);

/// Which of the five core tags count toward the readout. Per-check rather
/// than one dial: a bootleg library has no years and a classical one files
/// by composer.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CountedChecks {
    pub title: bool,
    pub artist: bool,
    pub album: bool,
    pub genre: bool,
    pub year: bool,
}

impl Default for CountedChecks {
    fn default() -> Self {
        CountedChecks {
            title: true,
            artist: true,
            album: true,
            genre: true,
            year: true,
        }
    }
}

impl CountedChecks {
    fn on(&self, check: Check) -> bool {
        match check {
            Check::Title => self.title,
            Check::Artist => self.artist,
            Check::Album => self.album,
            Check::Genre => self.genre,
            Check::Year => self.year,
        }
    }

    fn flip(&mut self, check: Check) {
        let field = match check {
            Check::Title => &mut self.title,
            Check::Artist => &mut self.artist,
            Check::Album => &mut self.album,
            Check::Genre => &mut self.genre,
            Check::Year => &mut self.year,
        };
        *field = !*field;
    }

    fn picked(&self) -> Vec<Check> {
        Check::ALL.into_iter().filter(|c| self.on(*c)).collect()
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HealthWidgetConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub checks: CountedChecks,
    pub show_percent: bool,
    pub open_on_click: bool,
}

impl Default for HealthWidgetConfig {
    fn default() -> Self {
        HealthWidgetConfig {
            chrome: PanelChrome::default(),
            checks: CountedChecks::default(),
            show_percent: true,
            open_on_click: true,
        }
    }
}

pub struct HealthWidgetPanel {
    state: AppState,
    config: HealthWidgetConfig,
    health: Completeness,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// Held rather than detached, so a burst of events replaces the pending
    /// walk and a closed panel takes its walk with it.
    scan: Option<Task<()>>,
    scan_generation: u64,
    _library_changed: Subscription,
}

impl HealthWidgetPanel {
    pub fn new(state: AppState, config: HealthWidgetConfig, cx: &mut Context<Self>) -> Self {
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Updated) {
                    this.refresh(cx);
                }
            },
        );
        let mut this = HealthWidgetPanel {
            state,
            config,
            health: Completeness::default(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            scan: None,
            scan_generation: 0,
            _library_changed,
        };
        this.refresh(cx);
        this
    }

    /// Re-walk the projection on the background executor, repainting only
    /// when a number moved. A result overtaken by another edit is dropped by
    /// generation. No drill-down ids are kept (cap zero): those are the
    /// health window's.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.scan_generation += 1;
        let generation = self.scan_generation;
        // The first walk skips the wait, or the panel shows a hundred percent
        // for a fifth of a second.
        let settle = (generation > 1).then_some(SCAN_DEBOUNCE);
        self.scan = Some(cx.spawn(async move |this, cx| {
            if let Some(settle) = settle {
                cx.background_executor().timer(settle).await;
            }
            let Ok(Some(projection)) = this.update(cx, |this, cx| {
                (this.scan_generation == generation)
                    .then(|| this.state.library.read(cx).projection().cloned())
            }) else {
                return;
            };
            let health = match projection {
                Some(projection) => {
                    cx.background_executor()
                        .spawn(async move { health::completeness(&projection, 0) })
                        .await
                }
                None => Completeness::default(),
            };
            this.update(cx, |this, cx| {
                if this.scan_generation != generation || health == this.health {
                    return;
                }
                this.health = health;
                cx.notify();
            })
            .ok();
        }));
    }

    fn percent(&self) -> f64 {
        (self.health.share_within(&self.config.checks.picked()) as f64 * 100.).round()
    }

    fn rows(&self) -> Vec<TooltipRow> {
        Check::ALL
            .into_iter()
            .map(|check| TooltipRow {
                label: check_label(check),
                missing: SharedString::from(rox_i18n::format::format_int(
                    self.health.missing(check).count as i64,
                )),
                counted: self.config.checks.on(check),
            })
            .collect()
    }

    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu = menu
            .separator()
            .label(rox_i18n::t!("health-readout-section"));
        // Live ticks through follow_panel + check_row: the flyout stays open on
        // a pick, so a tick baked in at build time would go stale.
        let panel = cx.entity();
        let submenu = PopupMenu::build(window, cx, move |mut submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            submenu = submenu.check_side(Side::Right);
            for check in Check::ALL {
                submenu = submenu.item(panel::check_row(
                    check_label(check),
                    None,
                    move |this: &Self| this.config.checks.on(check),
                    move |this, cx| {
                        this.config.checks.flip(check);
                        cx.notify();
                    },
                    &panel,
                ));
            }
            submenu
        });
        let menu = menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("health-checks-menu"),
            submenu,
        ));
        let menu = self.toggle_item(
            menu,
            rox_i18n::t!("health-show-percent"),
            self.config.show_percent,
            cx,
            |config| config.show_percent = !config.show_percent,
        );
        self.toggle_item(
            menu,
            rox_i18n::t!("health-click-opens"),
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
        flip: impl Fn(&mut HealthWidgetConfig) + 'static,
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
}

fn check_label(check: Check) -> SharedString {
    match check {
        Check::Title => rox_i18n::t!("health-tile-title"),
        Check::Artist => rox_i18n::t!("health-tile-artist"),
        Check::Album => rox_i18n::t!("health-tile-album"),
        Check::Genre => rox_i18n::t!("health-tile-genre"),
        Check::Year => rox_i18n::t!("health-tile-year"),
    }
}

struct TooltipRow {
    label: SharedString,
    missing: SharedString,
    counted: bool,
}

/// Opaque like the popup menus: it floats over panel content with no backdrop.
struct HealthTooltip {
    rows: Vec<TooltipRow>,
}

impl Render for HealthTooltip {
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
                    .child(rox_i18n::t!("health-tooltip-missing")),
            )
            .children(self.rows.iter().map(|row| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap(tokens::SPACE_MD)
                    .text_color(if row.counted {
                        palette::text_bright()
                    } else {
                        palette::text_faint()
                    })
                    .child(div().min_w_0().truncate().child(row.label.clone()))
                    .child(div().flex_none().child(row.missing.clone()))
            }))
    }
}

impl PanelSettings for HealthWidgetPanel {
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
        let mut checks = div().flex().flex_col().gap(tokens::SPACE_MD).child(
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(rox_i18n::t!("health-checks-menu.description")),
        );
        for check in Check::ALL {
            checks = checks.child(setting_row(
                check_label(check),
                None,
                toggle(
                    self.config.checks.on(check),
                    move |this: &mut Self, on, cx| {
                        if this.config.checks.on(check) != on {
                            this.config.checks.flip(check);
                        }
                        cx.notify();
                    },
                    cx,
                ),
            ));
        }
        Some(
            div()
                .flex()
                .flex_col()
                .gap(settings_ui::SECTION_GAP)
                .child(settings_ui::section(
                    rox_i18n::t!("health-checks-menu"),
                    None,
                    checks,
                ))
                .child(settings_ui::section(
                    rox_i18n::t!("health-readout-section"),
                    None,
                    setting_row(
                        rox_i18n::t!("health-show-percent"),
                        Some(rox_i18n::t!("health-show-percent.description")),
                        toggle(
                            self.config.show_percent,
                            |this: &mut Self, on, cx| {
                                this.config.show_percent = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    ),
                ))
                .child(settings_ui::section(
                    rox_i18n::t!("health-click-section"),
                    None,
                    setting_row(
                        rox_i18n::t!("health-open-on-click"),
                        Some(rox_i18n::t!("health-open-on-click.description")),
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

impl EventEmitter<PanelEvent> for HealthWidgetPanel {}

impl Focusable for HealthWidgetPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for HealthWidgetPanel {
    fn panel_name(&self) -> &'static str {
        "health widget"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("health-widget-title"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn locked(&self, _cx: &App) -> bool {
        self.config.chrome.locked
    }

    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        // "100%" is the widest the readout gets; the bare icon keeps the
        // strip's own minimum.
        let mut width = rox_dock::resizable::PANEL_MIN_SIZE;
        if self.config.show_percent {
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
            PopupMenuItem::new(rox_i18n::t!("health-open"))
                .icon(Icon::default().path(icons::ACTIVITY))
                .on_click(move |_, _, cx| {
                    rox_panel_api::openers::health_window(state.clone(), cx);
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
                HealthWidgetPanel::new(state, config, cx)
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

impl Render for HealthWidgetPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        let percent = self.percent();
        let show_percent = self.config.show_percent;
        // An empty library has no coverage to report, so the readout steps
        // back rather than claiming a hundred.
        let measured = self.health.tracks > 0;
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
                        .id("health-widget")
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
                                    rox_panel_api::openers::health_window(state, cx);
                                }
                            })
                        })
                        .child(
                            svg()
                                .path(icons::ACTIVITY)
                                .size(px(16.))
                                .flex_none()
                                .text_color(if measured {
                                    palette::text()
                                } else {
                                    palette::text_muted()
                                }),
                        )
                        .when(show_percent, |d| {
                            d.child(
                                div()
                                    // The strip is short, and "100%" would
                                    // otherwise wrap.
                                    .whitespace_nowrap()
                                    .text_xs()
                                    .text_color(if measured {
                                        palette::text()
                                    } else {
                                        palette::text_muted()
                                    })
                                    .child(SharedString::from(rox_i18n::format::format_percent(
                                        percent,
                                    ))),
                            )
                        })
                        .tooltip(move |_window, cx| {
                            let rows = weak
                                .upgrade()
                                .map(|this| this.read(cx).rows())
                                .unwrap_or_default();
                            cx.new(|_| HealthTooltip { rows }).into()
                        }),
                )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn health(complete: u64, missing: &[u8]) -> Completeness {
        let mut out = Completeness::default();
        for _ in 0..complete {
            out.add_row(0);
        }
        for mask in missing {
            out.add_row(*mask);
        }
        out
    }

    #[test]
    fn dropping_a_check_stops_it_counting_against_the_number() {
        let mut config = HealthWidgetConfig::default();
        // Four of five tracks complete, the fifth missing only its year.
        let coverage = health(4, &[Check::Year.bit()]);
        assert_eq!(
            (coverage.share_within(&config.checks.picked()) * 100.).round(),
            80.
        );
        config.checks.flip(Check::Year);
        assert_eq!(
            (coverage.share_within(&config.checks.picked()) * 100.).round(),
            100.
        );
    }

    #[test]
    fn a_row_failing_two_checks_is_counted_once() {
        let mut checks = CountedChecks::default();
        let coverage = health(1, &[Check::Genre.bit() | Check::Year.bit()]);
        assert_eq!(coverage.share_within(&checks.picked()), 0.5);
        checks.flip(Check::Year);
        assert_eq!(coverage.share_within(&checks.picked()), 0.5);
        checks.flip(Check::Genre);
        assert_eq!(coverage.share_within(&checks.picked()), 1.0);
    }

    #[test]
    fn every_check_off_counts_the_whole_library() {
        let mut checks = CountedChecks::default();
        for check in Check::ALL {
            checks.flip(check);
        }
        assert!(checks.picked().is_empty());
        assert_eq!(
            health(0, &[Check::Year.bit(); 5]).share_within(&checks.picked()),
            1.0
        );
    }

    #[test]
    fn an_empty_library_reads_as_complete() {
        let coverage = health(0, &[]);
        assert_eq!(coverage.share_within(&Check::ALL), 1.0);
        assert_eq!(coverage.tracks, 0);
    }
}
