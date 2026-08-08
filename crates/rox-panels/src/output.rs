//! The output panel: what the device actually agreed to, kept on screen
//! instead of behind the Audio settings page. Same readout the settings
//! window's status block draws and the track info chip abbreviates, in a
//! panel a layout can park somewhere permanent. ADR 19 is blunt that a
//! bit-perfect claim nobody checked is decoration, so every line here is
//! the negotiated stream talking back: the mode that's running, the rate
//! the card landed on, and whether anything is converting on the way out.

use gpui::{
    div, prelude::*, px, App, Context, Div, EventEmitter, FocusHandle, Focusable, Rgba,
    ScrollHandle, SharedString, Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_playback::output::{Mode, Negotiated};
use serde::{Deserialize, Serialize};

use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings, Tone};
use crate::panel_settings;
use crate::player::{OutputStatus, Player};

/// How much of the readout the panel draws.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputDetail {
    /// A chip: the mode, the rate, the format, and nothing else. Small
    /// enough to tuck into a corner of a layout and still be read at a
    /// glance, which is what this panel is for most of the time. The
    /// sentence it stands in for is a hover away.
    #[default]
    Badge,
    /// The headline alone on one colored line, the track info chip's
    /// weight. Fits a strip; the reasons live in the other one.
    Compact,
    /// The full callout: the headline, then every line the state earns.
    Expanded,
}

/// The output panel's per-view config: what a saved layout restores, and
/// what the settings window edits. Missing fields take the defaults, so a
/// layout dumped before a knob existed still loads.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub detail: OutputDetail,
    /// Name the running device in the headline. Off keeps the line to the
    /// mode and the numbers, which is all a one-device machine needs.
    pub device: bool,
    /// The quiet all-clear: the playing file's own rate, confirming nothing
    /// is converting it.
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
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _output_changed: Subscription,
}

impl OutputPanel {
    pub fn new(state: AppState, config: OutputConfig, cx: &mut Context<Self>) -> Self {
        // Two things move this panel: what the stream negotiated, and the
        // failure standing in when nothing opened. `player::observe_output`
        // watches the first alone, and an open that fails while nothing was
        // playing never moves the status off None, so the error rides along
        // in the same comparison. The clock, the volume, and the queue move
        // the player without moving this panel.
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
            focus: cx.focus_handle(),
            tab_panel: None,
            _output_changed,
        }
    }

    /// What the device agreed to, as a tone, a headline, and the lines the
    /// state earns; [`OutputStatus::lines`] carries the reasoning behind
    /// each line.
    fn readout(&self, cx: &App) -> (Tone, SharedString, Vec<SharedString>) {
        let player = self.state.player.read(cx);
        let Some(status) = player.output_status() else {
            // No stream and an error means the last open failed, which is a
            // different thing from an idle player and shouldn't read the
            // same: one is waiting, the other is broken.
            return match player.error() {
                Some(error) => (
                    Tone::Bad,
                    "No output".into(),
                    vec![error, "Pick another device, or turn exclusive off".into()],
                ),
                None => (
                    Tone::Info,
                    "Nothing playing".into(),
                    vec!["Start a track and this says what the device agreed to".into()],
                ),
            };
        };
        let negotiated = &status.negotiated;
        let (tone, _) = tone_for(&status);
        let mode = match negotiated.mode {
            Mode::Exclusive => "Exclusive",
            Mode::Shared => "Shared",
        };
        let numbers = format!(
            "{} Hz, {} ch, {}",
            negotiated.sample_rate, negotiated.channels, negotiated.format
        );
        let headline = if self.config.device {
            format!("{mode} on {}, {numbers}", negotiated.device)
        } else {
            format!("{mode}, {numbers}")
        };
        // The compact register: the reasons folded into one comma line, so
        // the expanded callout stays two lines tall in a docked slot. The
        // settings window's status block asks for the full sentences.
        let lines = status.lines(false, self.config.source_rate);
        (tone, headline.into(), lines)
    }

    /// The labelled detail modes, the settings row's and the flyout's one
    /// list.
    const DETAIL_PICKS: [(&'static str, OutputDetail); 3] = [
        ("Badge", OutputDetail::Badge),
        ("Compact", OutputDetail::Compact),
        ("Expanded", OutputDetail::Expanded),
    ];

    /// The panel's own dropdown entries: the detail pick and the two line
    /// toggles the settings window also carries, for a quick flip.
    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let entity = cx.entity();
        let panel = entity.clone();
        let submenu = PopupMenu::build(window, cx, move |submenu, _, cx| {
            // Follow the panel so the picked row's tick swaps live, the
            // source flyout's rule.
            panel::follow_panel(&panel, cx);
            let mut submenu = submenu.check_side(gpui_component::Side::Right);
            for (label, detail) in Self::DETAIL_PICKS {
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
        let menu = menu.item(PopupMenuItem::submenu("Detail", submenu));
        let toggle = |menu: PopupMenu, label: &'static str, checked, set: fn(&mut OutputConfig)| {
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
        let menu = toggle(menu, "Device Name", self.config.device, |c| {
            c.device = !c.device
        });
        toggle(menu, "File Rate", self.config.source_rate, |c| {
            c.source_rate = !c.source_rate
        })
    }
}

/// What a repaint hangs on: the negotiated stream, and the failure that
/// stands in when there isn't one.
fn watched(player: &Player) -> (Option<OutputStatus>, Option<SharedString>) {
    (player.output_status(), player.error())
}

/// The callout's tone, and whether the device is running at a rate the file
/// isn't. The two bad cases aren't the same size. A claim that failed is a
/// setting that didn't take, which is an error: exclusive is switched on and
/// you are not hearing it. Resampling is the mode working and still not
/// being bit-perfect, which is worth flagging without crying wolf.
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

/// [`Tone`] keeps its own color to itself, so the compact line maps the four
/// tones onto the palette roles the callout paints with. Info reads as the
/// quiet state rather than a color, the way the track info chip stays muted
/// until something is worth interrupting for.
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
                    "Detail",
                    Some(
                        "Badge keeps it to a chip with the rest on hover; compact gives the \
                         headline a line of its own, for a strip along an edge; expanded adds \
                         the reasons beside it, or under it when the panel is too narrow",
                    ),
                    panel::choices(
                        &Self::DETAIL_PICKS,
                        self.config.detail,
                        |this: &mut Self, detail, cx| {
                            this.config.detail = detail;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
                .child(panel::setting_row(
                    "Device Name",
                    Some(
                        "Name the running device in the headline; off keeps the line to the \
                         mode, the rate, and the format",
                    ),
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
                    "File Rate",
                    Some(
                        "Confirm the playing file's own rate when nothing is converting it. A \
                         conversion says so either way, since that's what the warning is about",
                    ),
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

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Output")
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
        )
    }
}

impl Render for OutputPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

impl OutputPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let (tone, headline, lines) = self.readout(cx);
        // Centering lives on this column rather than on the scroll box: a
        // percent height collapses inside an overflow_y_scroll, so the box
        // takes a max instead and this holds it in the middle.
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
                    // Nothing negotiated: the headline is already two words
                    // and says it better than any abbreviation would.
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

/// The badge's line: the headline squeezed to a chip. The rate goes to kHz
/// and the device and the channel count drop out, because a badge is the
/// glance and all three are a hover away.
fn badge_label(negotiated: &Negotiated) -> SharedString {
    let mode = match negotiated.mode {
        Mode::Exclusive => "Exclusive",
        Mode::Shared => "Shared",
    };
    format!(
        "{mode} {:.1} kHz {}",
        negotiated.sample_rate as f32 / 1000.0,
        negotiated.format
    )
    .into()
}

/// The badge's hover note: the callout it's standing in for, headline and
/// all. The chip is deliberately too small to say why it's colored, so this
/// is where the reason lives.
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

    use super::{badge_label, tone_for, OutputConfig, OutputDetail, OutputStatus, Tone};

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

    /// The three-way the settings page reads by: a refused claim is an
    /// error, a conversion is a warning, and a match is the good outcome.
    /// A rate nobody has read yet can't be resampling.
    #[test]
    fn tone_ranks_a_refused_claim_above_a_conversion() {
        assert!(tone_for(&status(Some("device busy"), 48000, Some(48000))) == (Tone::Bad, false));
        assert!(tone_for(&status(Some("device busy"), 48000, Some(44100))) == (Tone::Bad, true));
        assert!(tone_for(&status(None, 48000, Some(44100))) == (Tone::Warn, true));
        assert!(tone_for(&status(None, 44100, Some(44100))) == (Tone::Good, false));
        assert!(tone_for(&status(None, 44100, None)) == (Tone::Good, false));
    }

    /// A layout with no fields of ours loads as the badge with both lines
    /// on, and a saved one round-trips.
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

    /// The chip says the mode and the numbers that change between files,
    /// and nothing a hover can carry instead.
    #[test]
    fn the_badge_drops_everything_but_the_mode_and_the_numbers() {
        let mut negotiated = status(None, 44100, None).negotiated;
        assert_eq!(badge_label(&negotiated), "Exclusive 44.1 kHz s32");
        negotiated.mode = Mode::Shared;
        negotiated.sample_rate = 48000;
        assert_eq!(badge_label(&negotiated), "Shared 48.0 kHz s32");
    }
}
