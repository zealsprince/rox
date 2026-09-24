//! The output panel: what the device actually accepted, the settings
//! window's status readout in a panel a layout can park. Every line comes
//! from the negotiated stream (ADR 19): the running mode, the settled rate,
//! and whether anything converts on the way out.

use gpui::{
    App, Context, Div, EventEmitter, FocusHandle, Focusable, Rgba, ScrollHandle, SharedString,
    Subscription, WeakEntity, Window, div, prelude::*, px,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_playback::output::{Mode, Negotiated};
use serde::{Deserialize, Serialize};

use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings, Tone};
use crate::panel_settings;
use crate::player::{OutputStatus, Player};

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputDetail {
    /// The mode, rate, and format as a chip; the full sentence is a hover away.
    #[default]
    Badge,
    /// The headline alone on one colored line.
    Compact,
    Expanded,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub detail: OutputDetail,
    pub device: bool,
    /// The playing file's own rate, confirming nothing converts it.
    pub source_rate: bool,
}

impl Default for OutputConfig {
    fn default() -> Self {
        OutputConfig {
            chrome: PanelChrome::default(),
            detail: OutputDetail::default(),
            device: true,
            source_rate: true,
        }
    }
}

pub struct OutputPanel {
    state: AppState,
    config: OutputConfig,
    scroll: ScrollHandle,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _output_changed: Subscription,
}

impl OutputPanel {
    pub fn new(state: AppState, config: OutputConfig, cx: &mut Context<Self>) -> Self {
        // Repaint on the negotiated stream or the open error.
        // `player::observe_output` watches only the first, and a failed open
        // leaves the status at None.
        let mut last = watched(state.player.read(cx));
        let _output_changed = cx.observe(&state.player, move |_, player, cx| {
            let now = watched(player.read(cx));
            if now != last {
                last = now;
                cx.notify();
            }
        });
        OutputPanel {
            state,
            config,
            scroll: ScrollHandle::default(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            _output_changed,
        }
    }

    fn readout(&self, cx: &App) -> (Tone, SharedString, Vec<SharedString>) {
        let player = self.state.player.read(cx);
        let Some(status) = player.output_status() else {
            // No stream plus an error is a failed open, not an idle player.
            return match player.error() {
                Some(error) => (
                    Tone::Bad,
                    rox_i18n::t!("output-no-output"),
                    vec![error, rox_i18n::t!("output-pick-another-device")],
                ),
                None => (
                    Tone::Info,
                    rox_i18n::t!("output-nothing-playing"),
                    vec![rox_i18n::t!("output-start-track-hint")],
                ),
            };
        };
        let negotiated = &status.negotiated;
        let (tone, _) = tone_for(&status);
        let mode = match negotiated.mode {
            Mode::Exclusive => rox_i18n::t_static("output-mode-exclusive"),
            Mode::Shared => rox_i18n::t_static("output-mode-shared"),
        };
        let headline = if self.config.device {
            rox_i18n::t!(
                "output-headline-device",
                mode = mode,
                device = negotiated.device.clone(),
                rate = negotiated.sample_rate,
                channels = negotiated.channels,
                format = negotiated.format.to_string()
            )
        } else {
            rox_i18n::t!(
                "output-headline",
                mode = mode,
                rate = negotiated.sample_rate,
                channels = negotiated.channels,
                format = negotiated.format.to_string()
            )
        };
        // The compact register, so the callout stays two lines tall in a dock.
        let lines = status.lines(false, self.config.source_rate);
        (tone, headline, lines)
    }

    fn detail_picks() -> [(SharedString, OutputDetail); 3] {
        [
            (rox_i18n::t!("output-detail-badge"), OutputDetail::Badge),
            (rox_i18n::t!("output-detail-compact"), OutputDetail::Compact),
            (
                rox_i18n::t!("output-detail-expanded"),
                OutputDetail::Expanded,
            ),
        ]
    }

    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let entity = cx.entity();
        let panel = entity.clone();
        let submenu = PopupMenu::build(window, cx, move |submenu, _, cx| {
            panel::follow_panel(&panel, cx);
            let mut submenu = submenu.check_side(gpui_component::Side::Right);
            for (label, detail) in Self::detail_picks() {
                submenu = submenu.item(panel::check_row(
                    label,
                    None,
                    move |this: &Self| this.config.detail == detail,
                    move |this, cx| {
                        this.config.detail = detail;
                        cx.notify();
                    },
                    &panel,
                ));
            }
            submenu
        });
        let menu = menu.item(PopupMenuItem::submenu(
            rox_i18n::t!("output-detail-label"),
            submenu,
        ));
        let toggle = |menu: PopupMenu, label: SharedString, checked, set: fn(&mut OutputConfig)| {
            let weak = entity.downgrade();
            menu.item(
                PopupMenuItem::new(label)
                    .checked(checked)
                    .on_click(move |_, _, cx| {
                        let Some(this) = weak.upgrade() else { return };
                        this.update(cx, |this, cx| {
                            set(&mut this.config);
                            cx.notify();
                        });
                    }),
            )
        };
        let menu = toggle(
            menu,
            rox_i18n::t!("output-device-name"),
            self.config.device,
            |c| c.device = !c.device,
        );
        toggle(
            menu,
            rox_i18n::t!("output-file-rate"),
            self.config.source_rate,
            |c| c.source_rate = !c.source_rate,
        )
    }
}

fn watched(player: &Player) -> (Option<OutputStatus>, Option<SharedString>) {
    (player.output_status(), player.error())
}

/// A refused exclusive claim is Bad; resampling is only Warn.
fn tone_for(status: &OutputStatus) -> (Tone, bool) {
    let resampling = status
        .source_rate
        .is_some_and(|source| source != status.negotiated.sample_rate);
    let tone = if status.negotiated.fallback.is_some() {
        Tone::Bad
    } else if resampling {
        Tone::Warn
    } else {
        Tone::Good
    };
    (tone, resampling)
}

fn tone_color(tone: Tone) -> Rgba {
    match tone {
        Tone::Info => palette::text_muted(),
        Tone::Good => palette::tone_good(),
        Tone::Warn => palette::tone_warn(),
        Tone::Bad => palette::tone_bad(),
    }
}

impl PanelSettings for OutputPanel {
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

    fn behavior(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        Some(
            div()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_MD)
                .child(panel::setting_row(
                    rox_i18n::t!("output-detail-label"),
                    Some(rox_i18n::t!("output-detail-label.description")),
                    panel::choices_shared(
                        &Self::detail_picks(),
                        self.config.detail,
                        |this: &mut Self, detail, cx| {
                            this.config.detail = detail;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .child(panel::setting_row(
                    rox_i18n::t!("output-device-name"),
                    Some(rox_i18n::t!("output-device-name.description")),
                    panel::toggle(
                        self.config.device,
                        |this: &mut Self, on, cx| {
                            this.config.device = on;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .child(panel::setting_row(
                    rox_i18n::t!("output-file-rate"),
                    Some(rox_i18n::t!("output-file-rate.description")),
                    panel::toggle(
                        self.config.source_rate,
                        |this: &mut Self, on, cx| {
                            this.config.source_rate = on;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .into_any_element(),
        )
    }
}

impl EventEmitter<PanelEvent> for OutputPanel {}

impl Focusable for OutputPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for OutputPanel {
    fn panel_name(&self) -> &'static str {
        "output"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("output-title"),
        )
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

    fn min_size(&self, _cx: &App) -> gpui::Size<gpui::Pixels> {
        crate::panel::chrome_min_size(
            &self.config.chrome,
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
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
                OutputPanel::new(state, config, cx)
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

impl Render for OutputPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(cx).track_focus(&focus))
    }
}

impl OutputPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let (tone, headline, lines) = self.readout(cx);
        // Center on this column, not the scroll box: a percent height collapses
        // inside overflow_y_scroll.
        let root = div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .flex_col()
            .justify_center()
            .p(tokens::SPACE_MD);
        match self.config.detail {
            OutputDetail::Badge => {
                let label = self
                    .state
                    .player
                    .read(cx)
                    .output_status()
                    .map(|status| badge_label(&status.negotiated))
                    .unwrap_or_else(|| headline.clone());
                let note = BadgeNote { headline, lines };
                root.items_center().child(
                    div()
                        .id("output-badge")
                        .flex_none()
                        .max_w_full()
                        .truncate()
                        .px(tokens::SPACE_SM)
                        .rounded(tokens::RADIUS)
                        .bg(palette::bg_control())
                        .text_xs()
                        .text_color(tone_color(tone))
                        .child(label)
                        .tooltip(move |_, cx| cx.new(|_| note.clone()).into()),
                )
            }
            OutputDetail::Compact => root.child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(tone_color(tone))
                    .child(headline),
            ),
            OutputDetail::Expanded => root.child(
                div()
                    .id("output-callout")
                    .w_full()
                    .max_h_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(panel::banner_flow(tone, headline, lines)),
            ),
        }
    }
}

fn badge_label(negotiated: &Negotiated) -> SharedString {
    let mode = match negotiated.mode {
        Mode::Exclusive => rox_i18n::t_static("output-mode-exclusive"),
        Mode::Shared => rox_i18n::t_static("output-mode-shared"),
    };
    format!(
        "{mode} {} {}",
        rox_i18n::format::format_unit(f64::from(negotiated.sample_rate) / 1000.0, 1, "kHz"),
        negotiated.format
    )
    .into()
}

#[derive(Clone)]
struct BadgeNote {
    headline: SharedString,
    lines: Vec<SharedString>,
}

impl Render for BadgeNote {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .p(tokens::SPACE_SM)
            .max_w(px(320.))
            .rounded(tokens::RADIUS)
            .border_1()
            .border_color(palette::border())
            .bg(palette::bg_menu_opaque())
            .shadow_md()
            .text_xs()
            .text_color(palette::text())
            .child(self.headline.clone())
            .children(
                self.lines
                    .iter()
                    .map(|line| div().text_color(palette::text_muted()).child(line.clone())),
            )
    }
}

#[cfg(test)]
mod tests {
    use rox_playback::output::{Mode, Negotiated};

    use super::{OutputConfig, OutputDetail, OutputStatus, Tone, badge_label, tone_for};

    fn status(fallback: Option<&str>, device_rate: u32, source_rate: Option<u32>) -> OutputStatus {
        OutputStatus {
            negotiated: Negotiated {
                mode: Mode::Exclusive,
                device: "Test Card".into(),
                sample_rate: device_rate,
                channels: 2,
                format: "s32".into(),
                fallback: fallback.map(str::to_string),
            },
            source_rate,
            leveling_db: None,
        }
    }

    #[test]
    fn tone_ranks_a_refused_claim_above_a_conversion() {
        assert!(tone_for(&status(Some("device busy"), 48000, Some(48000))) == (Tone::Bad, false));
        assert!(tone_for(&status(Some("device busy"), 48000, Some(44100))) == (Tone::Bad, true));
        assert!(tone_for(&status(None, 48000, Some(44100))) == (Tone::Warn, true));
        assert!(tone_for(&status(None, 44100, Some(44100))) == (Tone::Good, false));
        assert!(tone_for(&status(None, 44100, None)) == (Tone::Good, false));
    }

    #[test]
    fn missing_fields_default_to_the_badge() {
        let config: OutputConfig = serde_json::from_str("{}").unwrap();
        assert!(config.detail == OutputDetail::Badge);
        assert!(config.device);
        assert!(config.source_rate);

        let config = OutputConfig {
            detail: OutputDetail::Expanded,
            device: false,
            ..OutputConfig::default()
        };
        let saved = serde_json::to_value(&config).unwrap();
        let back: OutputConfig = serde_json::from_value(saved).unwrap();
        assert!(back.detail == OutputDetail::Expanded);
        assert!(!back.device);
        assert!(back.source_rate);
    }

    #[test]
    fn the_badge_drops_everything_but_the_mode_and_the_numbers() {
        let mut negotiated = status(None, 44100, None).negotiated;
        assert_eq!(badge_label(&negotiated), "Exclusive 44.1 kHz s32");
        negotiated.mode = Mode::Shared;
        negotiated.sample_rate = 48000;
        assert_eq!(badge_label(&negotiated), "Shared 48.0 kHz s32");
    }
}
