//! The health widget: the library's tag coverage boiled down to one
//! percentage for a transport row, and the health window's front door.
//! Hovering breaks it out per tag, so the number is a glance away without
//! giving up a panel-sized surface.
//!
//! The number is [`rox_library::health::completeness`]'s, the same walk the
//! health window's overview ring reads, so the two can't drift. It runs when
//! the catalog changes and never per frame, the library's own read cadence.
//!
//! What this deliberately doesn't do is measure anything that costs a file
//! read. Album art, duplicates and album gaps belong to the health window's
//! background pass, where a user has asked for them and can watch them land;
//! a widget sitting in a transport row has no business probing a library's
//! worth of files because it happened to get docked.

use std::time::Duration;

use gpui::{
    div, prelude::*, px, svg, AnyElement, App, Context, EventEmitter, FocusHandle, Focusable,
    SharedString, Subscription, Task, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Side};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::health::{self, Check, Completeness};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::panel::{self, setting_row, toggle, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::settings::ui as settings_ui;

/// How long the widget waits before re-walking. A catalog change raises
/// `LibraryEvent::Updated` at the start of the reload and again at the end,
/// and a running scan raises one per interim batch, so a single edit
/// arrives as a burst. A percentage in a transport row has no business
/// measuring a library once per event on the way through.
const SCAN_DEBOUNCE: Duration = Duration::from_millis(200);

/// Which of the five core tags count toward the readout.
///
/// A per-check switch rather than a single "strictness" dial: a library of
/// live bootlegs has no year worth tagging and a classical library files by
/// composer, and either owner would rather drop the check than read a number
/// that will never reach a hundred. Every check on by default, which is the
/// health window's own headline.
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

    /// The checks the readout counts, in listing order.
    fn picked(&self) -> Vec<Check> {
        Check::ALL.into_iter().filter(|c| self.on(*c)).collect()
    }
}

/// The widget's config: the shared chrome plus what it counts, whether the
/// percentage shows at all, and whether a click opens the health window.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HealthWidgetConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub checks: CountedChecks,
    /// Draw the percentage beside the icon. Off leaves a bare icon, for a
    /// strip that only needs the way in to the health window.
    pub show_percent: bool,
    /// Click the widget to open the health window. On by default; off leaves
    /// it a readout.
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
    /// The cached coverage, so a repaint never walks the projection.
    health: Completeness,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// The walk that's out, held rather than detached: a burst of library
    /// events replaces the pending one instead of queueing a walk per
    /// event, and a panel that goes away takes its walk with it.
    scan: Option<Task<()>>,
    /// Bumped per walk; a result carrying an older number is dropped.
    scan_generation: u64,
    /// A rescan, a retag or a rating write swaps the projection, which moves
    /// every number here.
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

    /// Re-walk the projection, repainting only when a number actually moved:
    /// a rating write swaps the projection without touching a single tag.
    ///
    /// The walk itself goes to the background executor over an Arc of the
    /// projection. It's O(live rows) and it used to run on the UI thread on
    /// every library event, which on a large library is a stall a docked
    /// widget has no right to cause; the old percentage stays on screen
    /// until the new one lands, and a result overtaken by another edit is
    /// dropped by generation.
    ///
    /// No drill-down ids are kept (the cap is zero): the doors are the health
    /// window's, and a widget holding a thousand ids per repaint would be
    /// paying for a feature it doesn't have.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.scan_generation += 1;
        let generation = self.scan_generation;
        // The first walk is the one the panel appears with, so it skips the
        // wait rather than showing a hundred percent for a fifth of a second.
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

    /// The readout as a whole percent over the counted checks.
    fn percent(&self) -> f64 {
        (self.health.share_within(&self.config.checks.picked()) as f64 * 100.).round()
    }

    /// The tooltip's rows: every check's missing count with the counted ones
    /// marked, read off the cache.
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

    /// The panel's own quick entries: the counted-tags flyout and the two
    /// readout toggles, so the widget can be re-aimed from its right-click
    /// without a trip through the settings window.
    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu = menu
            .separator()
            .label(rox_i18n::t!("health-readout-section"));
        // The checks as a flyout, with live ticks through follow_panel +
        // check_row: the flyout stays open on a pick, so a tick baked in at
        // build time would stay on the old set.
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

    /// One checked menu row over a config boolean.
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

/// A check's name, shared with the health window's overview rows.
fn check_label(check: Check) -> SharedString {
    match check {
        Check::Title => rox_i18n::t!("health-tile-title"),
        Check::Artist => rox_i18n::t!("health-tile-artist"),
        Check::Album => rox_i18n::t!("health-tile-album"),
        Check::Genre => rox_i18n::t!("health-tile-genre"),
        Check::Year => rox_i18n::t!("health-tile-year"),
    }
}

/// One check's line in the tooltip.
struct TooltipRow {
    label: SharedString,
    missing: SharedString,
    /// Whether this check is one the readout counts, which reads brighter
    /// than the rest.
    counted: bool,
}

/// The hover tooltip: every check's missing count, so the one percentage has
/// something behind it. Opaque fill like the popup menus, since it floats
/// over panel content with no backdrop behind it.
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
        // The explainer belongs to the set rather than to any one switch,
        // so it leads the section instead of hanging off a row.
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
        // "100%" is the widest the readout gets, so the floor widens by
        // about that; the bare icon keeps the strip's own minimum.
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
        // A library with nothing in it has no coverage to report, so the
        // readout steps back rather than claiming a perfect hundred.
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

    /// A stand-in for a walked projection: rows folded in by the checks they
    /// fail, through the same arithmetic the real walk uses.
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

    /// The knob's whole point: dropping a check the library never had lifts
    /// the number to what the rest of the tags actually say.
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

    /// A row failing two checks is one incomplete row, not two: dropping one
    /// of the two still leaves it out, and dropping both brings it back.
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

    /// Every check off is not an error: it counts every track, since there's
    /// nothing left for one to fail.
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

    /// An empty library reads as a hundred rather than a zero, and the
    /// readout marks itself unmeasured so the strip doesn't imply otherwise.
    #[test]
    fn an_empty_library_reads_as_complete() {
        let coverage = health(0, &[]);
        assert_eq!(coverage.share_within(&Check::ALL), 1.0);
        assert_eq!(coverage.tracks, 0);
    }
}
