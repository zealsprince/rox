//! The volume strip panel: the speaker button that toggles mute, the
//! volume slider, and the percent readout, each its own toggle so the
//! strip composes down to any subset.

use gpui::{
    canvas, div, prelude::*, px, svg, AnyElement, App, Context, Div, EventEmitter, FocusHandle,
    Focusable, MouseButton, Pixels, Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::tooltip::Tooltip;
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{
    self, align_row, justify, Align, AppState, PanelChrome, PanelSettings, ScrubState,
};
use crate::panel_settings;
use crate::player::observe_view;

use super::{default_true, transport_panel};

/// The volume panel's per-view config: what a saved layout restores, and
/// what the settings window edits. Each piece of the strip is its own
/// toggle; the field attributes only matter to the serializer, since
/// deserialization routes through [`VolumeConfigDump`].
#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "VolumeConfigDump")]
pub struct VolumeConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub align: Align,
    /// Let the slider fill whatever width the panel has instead of capping
    /// at its natural size.
    pub stretch: bool,
    /// The speaker button that toggles mute. Scrolling anywhere on the
    /// strip still changes the volume, so the icon alone stays usable.
    pub icon: bool,
    /// The volume slider.
    pub slider: bool,
    /// The percent readout. While it is off the speaker icon carries the
    /// number in a tooltip instead.
    pub percent: bool,
}

impl Default for VolumeConfig {
    fn default() -> Self {
        VolumeConfig {
            chrome: PanelChrome::default(),
            align: Align::default(),
            stretch: false,
            icon: true,
            slider: true,
            percent: true,
        }
    }
}

/// The dump shape [`VolumeConfig`] deserializes through: the per-piece
/// toggles, plus the retired `icon_only` knob so layouts saved before the
/// pieces became toggles fold into icon on, slider and percent off.
#[derive(Deserialize)]
struct VolumeConfigDump {
    #[serde(flatten)]
    chrome: PanelChrome,
    #[serde(default)]
    align: Align,
    #[serde(default)]
    stretch: bool,
    #[serde(default)]
    icon_only: bool,
    #[serde(default = "default_true")]
    icon: bool,
    #[serde(default = "default_true")]
    slider: bool,
    #[serde(default = "default_true")]
    percent: bool,
}

impl From<VolumeConfigDump> for VolumeConfig {
    fn from(dump: VolumeConfigDump) -> Self {
        VolumeConfig {
            chrome: dump.chrome,
            align: dump.align,
            stretch: dump.stretch,
            icon: dump.icon || dump.icon_only,
            slider: dump.slider && !dump.icon_only,
            percent: dump.percent && !dump.icon_only,
        }
    }
}

/// The volume strip: the speaker button that toggles mute, the volume
/// slider, and the percent readout, each on its own config toggle.
pub struct VolumePanel {
    state: AppState,
    config: VolumeConfig,
    /// The slider's painted bounds and drag state.
    scrub: ScrubState,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
}

impl VolumePanel {
    pub fn new(state: AppState, config: VolumeConfig, cx: &mut Context<Self>) -> Self {
        // Volume and mute are not on the pump at all; the gated observe
        // still catches changes from a keyboard shortcut or elsewhere.
        let _player_changed = observe_view(&state.player, cx);
        VolumePanel {
            state,
            config,
            scrub: ScrubState::default(),
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
        }
    }

    /// The panel's own dropdown entries: the per-piece toggles and the
    /// stretch knob, the same rows the customize window edits.
    fn config_menu(&self, menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Icon")
                .checked(self.config.icon)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.icon = !this.config.icon;
                        cx.notify();
                    });
                }),
        );
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Slider")
                .checked(self.config.slider)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.slider = !this.config.slider;
                        cx.notify();
                    });
                }),
        );
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Percent")
                .checked(self.config.percent)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.percent = !this.config.percent;
                        cx.notify();
                    });
                }),
        );
        let weak = cx.entity().downgrade();
        menu.item(
            PopupMenuItem::new("Stretch")
                .disabled(!self.config.slider)
                .checked(self.config.stretch)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.stretch = !this.config.stretch;
                        cx.notify();
                    });
                }),
        )
    }
}

impl PanelSettings for VolumePanel {
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

    fn pages(&self) -> &'static [(&'static str, &'static str)] {
        &[("Layout", icons::ALIGN_LEFT)]
    }

    fn page(
        &mut self,
        _page: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(align_row(
                self.config.align,
                |this: &mut Self, align, cx| {
                    this.config.align = align;
                    cx.notify();
                },
                cx,
            ))
            .child(panel::setting_row(
                "Icon",
                Some("The speaker button that toggles mute"),
                panel::toggle(
                    self.config.icon,
                    |this: &mut Self, icon, cx| {
                        this.config.icon = icon;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Slider",
                Some("The volume slider"),
                panel::toggle(
                    self.config.slider,
                    |this: &mut Self, slider, cx| {
                        this.config.slider = slider;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(self.config.slider, |d| {
                d.child(panel::setting_row(
                    "Stretch",
                    Some("Let the slider fill the panel instead of capping its width"),
                    panel::toggle(
                        self.config.stretch,
                        |this: &mut Self, stretch, cx| {
                            this.config.stretch = stretch;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .child(panel::setting_row(
                "Percent",
                Some("The percent readout; off, the speaker's tooltip carries it"),
                panel::toggle(
                    self.config.percent,
                    |this: &mut Self, percent, cx| {
                        this.config.percent = percent;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .into_any_element()
    }
}

/// One wheel step over the volume panel, wherever the pointer sits on the
/// strip. A notch arrives as 3 lines, so one notch steps 5%; the range is
/// 0 to 100% and touching it unmutes.
fn volume_scroll(
    this: &mut VolumePanel,
    event: &gpui::ScrollWheelEvent,
    _window: &mut Window,
    cx: &mut Context<VolumePanel>,
) {
    let lines = match event.delta {
        gpui::ScrollDelta::Lines(lines) => lines.y,
        gpui::ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / 20.0,
    };
    this.state.player.update(cx, |player, cx| {
        let volume = (player.volume() + lines / 3.0 * 0.05).clamp(0.0, 1.0);
        player.set_volume(volume, cx);
    });
}

impl Render for VolumePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
    }
}

impl VolumePanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let player = self.state.player.read(cx);
        let volume = player.volume();
        let muted = player.muted();
        let percent = (volume * 100.0).round() as u32;

        // The speaker doubles as the mute toggle and the state readout:
        // crossed out while muted, fewer waves at low volume.
        let (speaker, speaker_color) = if muted {
            (icons::VOLUME_X, palette::text_faint())
        } else if volume <= 0.5 {
            (icons::VOLUME_1, palette::text())
        } else {
            (icons::VOLUME_2, palette::text())
        };

        // Click toggles mute; with the readout off, the percent rides
        // along in a tooltip so it still has a home.
        let icon = div()
            .id("volume-icon")
            .p(tokens::ICON_PAD)
            .rounded(tokens::RADIUS)
            .hover(|d| d.bg(palette::bg_control()))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.state
                        .player
                        .update(cx, |player, cx| player.toggle_mute(cx));
                }),
            )
            .when(!self.config.percent, |d| {
                d.tooltip(move |window, cx| Tooltip::new(format!("{percent}%")).build(window, cx))
            })
            .child(svg().path(speaker).size(px(16.)).text_color(speaker_color));

        let scrub = self.scrub.clone();
        let player = self.state.player.clone();
        let slider = div()
            .flex_1()
            .min_w(tokens::SLIDER_MIN_W)
            .when(!self.config.stretch, |d| d.max_w(tokens::SLIDER_MAX_W))
            .h(tokens::CONTROL_H)
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    this.scrub.begin();
                    if let Some(fraction) = this.scrub.fraction(event.position.x) {
                        this.state
                            .player
                            .update(cx, |player, cx| player.set_volume(fraction, cx));
                    }
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    {
                        let scrub = scrub.clone();
                        move |bounds, _, _| scrub.set_bounds(bounds)
                    },
                    // Muted keeps the knob where it is and dims the fill. The
                    // slider spans 0 to 100%; a louder hand-edited settings
                    // value shows as full.
                    move |bounds, _, window, _| {
                        panel::paint_slider(volume, muted, bounds, window);
                        panel::scrub_on_paint(&scrub, window, {
                            let player = player.clone();
                            move |fraction, cx| {
                                player.update(cx, |player, cx| player.set_volume(fraction, cx))
                            }
                        });
                    },
                )
                .size_full(),
            );

        div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .items_center()
            .map(|d| justify(d, self.config.align))
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_MD)
            // Scrolling anywhere on the strip nudges the volume; like the
            // slider it spans 0 to 100% and unmutes on touch.
            .on_scroll_wheel(cx.listener(volume_scroll))
            .when(self.config.icon, |d| d.child(icon))
            .when(self.config.slider, |d| d.child(slider))
            .when(self.config.percent, |d| {
                d.child(
                    div()
                        .w(px(40.))
                        .flex_none()
                        .text_center()
                        .text_color(palette::text_muted())
                        .child(format!("{percent}%")),
                )
            })
    }
}

// The volume strip is fully composable, so it leans on the app's own panel
// floor instead of pinning a width.
transport_panel!(
    VolumePanel,
    "volume",
    "Volume",
    min_w = |_: &VolumePanel| rox_dock::resizable::PANEL_MIN_SIZE
);

#[cfg(test)]
mod tests {
    use super::VolumeConfig;

    /// A layout saved before the pieces became toggles carries `icon_only`,
    /// which folds into icon on, slider and percent off.
    #[test]
    fn icon_only_folds_into_the_piece_toggles() {
        let config: VolumeConfig = serde_json::from_str(r#"{"icon_only": true}"#).unwrap();
        assert!(config.icon);
        assert!(!config.slider);
        assert!(!config.percent);
    }

    /// A layout with no piece fields at all decodes to the full strip.
    #[test]
    fn missing_toggles_default_on() {
        let config: VolumeConfig = serde_json::from_str("{}").unwrap();
        assert!(config.icon);
        assert!(config.slider);
        assert!(config.percent);
    }
}
