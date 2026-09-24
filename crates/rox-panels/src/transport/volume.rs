//! The volume strip panel: the speaker button that toggles mute, the
//! volume slider, and the percent readout, an ordered list so the strip
//! composes down to any subset in any order.

use gpui::{
    AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable, MouseButton, Pixels,
    Subscription, WeakEntity, Window, canvas, div, prelude::*, px, svg,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{
    self, Align, AppState, PanelChrome, PanelSettings, ScrubState, align_row, justify,
};
use crate::panel_settings;
use crate::player::observe_view;

use super::{default_true, transport_panel};

/// The retired side-picker positions. Legacy-only.
#[derive(Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum PiecePos {
    #[default]
    Left,
    Right,
    Hidden,
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VolumeItem {
    /// Scrolling anywhere on the strip still changes the volume, so the icon
    /// alone stays usable.
    Icon,
    Slider,
    /// While hidden, the icon's tooltip shows the level instead.
    Percent,
    Spacer,
}

/// Stock order: where a menu toggle slots a re-shown piece back in.
const ITEMS: &[panel::ArrangeSpec<VolumeItem>] = &[
    panel::ArrangeSpec {
        key: "volume-item-icon",
        icon: Some(icons::VOLUME_2),
        value: VolumeItem::Icon,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "volume-item-slider",
        icon: Some(icons::SLIDERS),
        value: VolumeItem::Slider,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "volume-item-percent",
        icon: None,
        value: VolumeItem::Percent,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "head-piece-spacer",
        icon: Some(icons::MOVE_HORIZONTAL),
        value: VolumeItem::Spacer,
        repeats: true,
    },
];

#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "VolumeConfigDump")]
pub struct VolumeConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub align: Align,
    pub stretch: bool,
    pub percent_db: bool,
    pub items: Vec<VolumeItem>,
}

impl Default for VolumeConfig {
    fn default() -> Self {
        VolumeConfig {
            chrome: PanelChrome::default(),
            align: Align::default(),
            stretch: false,
            percent_db: false,
            items: vec![VolumeItem::Icon, VolumeItem::Slider, VolumeItem::Percent],
        }
    }
}

/// An older layout's piece field: a bool, or the side picker. `true`
/// means the piece's stock side.
#[derive(Deserialize)]
#[serde(untagged)]
enum PieceDump {
    Legacy(bool),
    Pos(PiecePos),
}

impl PieceDump {
    fn fold(self, stock: PiecePos) -> PiecePos {
        match self {
            PieceDump::Legacy(true) => stock,
            PieceDump::Legacy(false) => PiecePos::Hidden,
            PieceDump::Pos(pos) => pos,
        }
    }
}

/// Reads the ordered list, the per-piece toggle or side forms, and the
/// retired `icon_only`.
#[derive(Deserialize)]
struct VolumeConfigDump {
    #[serde(flatten)]
    chrome: PanelChrome,
    #[serde(default)]
    align: Align,
    #[serde(default)]
    stretch: bool,
    #[serde(default)]
    percent_db: bool,
    #[serde(default)]
    items: Option<Vec<VolumeItem>>,
    #[serde(default)]
    icon_only: bool,
    #[serde(default = "piece_on")]
    icon: PieceDump,
    #[serde(default = "default_true")]
    slider: bool,
    #[serde(default = "piece_on")]
    percent: PieceDump,
}

fn piece_on() -> PieceDump {
    PieceDump::Legacy(true)
}

impl From<VolumeConfigDump> for VolumeConfig {
    fn from(dump: VolumeConfigDump) -> Self {
        let items = match dump.items {
            Some(items) => panel::dedup(ITEMS, items),
            None => {
                // Fold in the order the strip rendered: each piece on its side of the
                // slider.
                let icon = dump.icon.fold(PiecePos::Left);
                let icon = if dump.icon_only && icon == PiecePos::Hidden {
                    PiecePos::Left
                } else {
                    icon
                };
                let slider = dump.slider && !dump.icon_only;
                let percent = if dump.icon_only {
                    PiecePos::Hidden
                } else {
                    dump.percent.fold(PiecePos::Right)
                };
                let mut items = Vec::new();
                if icon == PiecePos::Left {
                    items.push(VolumeItem::Icon);
                }
                if percent == PiecePos::Left {
                    items.push(VolumeItem::Percent);
                }
                if slider {
                    items.push(VolumeItem::Slider);
                }
                if percent == PiecePos::Right {
                    items.push(VolumeItem::Percent);
                }
                if icon == PiecePos::Right {
                    items.push(VolumeItem::Icon);
                }
                items
            }
        };
        VolumeConfig {
            chrome: dump.chrome,
            align: dump.align,
            stretch: dump.stretch,
            percent_db: dump.percent_db,
            items,
        }
    }
}

pub struct VolumePanel {
    state: AppState,
    config: VolumeConfig,
    scrub: ScrubState,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// The strip before a menu toggle hid a piece, so re-showing it restores
    /// its place. Panel state, not config.
    items_stash: Option<Vec<VolumeItem>>,
    _player_changed: Subscription,
}

impl VolumePanel {
    pub fn new(state: AppState, config: VolumeConfig, cx: &mut Context<Self>) -> Self {
        // Volume isn't on the pump; the gated observe catches shortcut changes.
        let _player_changed = observe_view(&state.player, cx);
        VolumePanel {
            state,
            config,
            scrub: ScrubState::default(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            items_stash: None,
            _player_changed,
        }
    }

    fn config_menu(
        &self,
        menu: PopupMenu,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let mut menu = menu;
        for (name, value) in [
            (rox_i18n::t!("volume-item-icon"), VolumeItem::Icon),
            (rox_i18n::t!("volume-item-slider"), VolumeItem::Slider),
            (rox_i18n::t!("volume-item-percent"), VolumeItem::Percent),
        ] {
            let weak = cx.entity().downgrade();
            menu = menu.item(
                PopupMenuItem::new(name)
                    .checked(self.config.items.contains(&value))
                    .on_click(move |_, _, cx| {
                        let Some(this) = weak.upgrade() else { return };
                        this.update(cx, |this, cx| {
                            this.config.items = panel::toggled_stashed(
                                ITEMS,
                                &this.config.items,
                                &mut this.items_stash,
                                &[value],
                            );
                            cx.notify();
                        });
                    }),
            );
        }
        let weak = cx.entity().downgrade();
        menu.item(
            PopupMenuItem::new(rox_i18n::t!("volume-stretch"))
                .disabled(!self.config.items.contains(&VolumeItem::Slider))
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
            .child(panel::setting_block(
                rox_i18n::t!("volume-pieces"),
                Some(rox_i18n::t!("volume-pieces.description")),
                None,
                panel::arrange_editor(
                    "volume-items",
                    ITEMS,
                    &self.config.items,
                    |this: &mut Self, items, cx| {
                        this.config.items = items;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(self.config.items.contains(&VolumeItem::Slider), |d| {
                d.child(panel::setting_row(
                    rox_i18n::t!("volume-stretch"),
                    Some(rox_i18n::t!("volume-stretch.description")),
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
            .when(self.config.items.contains(&VolumeItem::Percent), |d| {
                d.child(panel::setting_row(
                    rox_i18n::t!("volume-readout"),
                    Some(rox_i18n::t!("volume-readout.description")),
                    panel::choices_shared(
                        &[
                            (rox_i18n::t!("volume-readout-percent"), false),
                            (rox_i18n::t!("volume-readout-decibels"), true),
                        ],
                        self.config.percent_db,
                        |this: &mut Self, percent_db, cx| {
                            this.config.percent_db = percent_db;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .into_any_element()
    }
}

/// 20 log10 of the linear gain; silence reads "-inf dB".
fn fmt_db(volume: f32) -> String {
    if volume <= 0.0 {
        "-inf dB".into()
    } else {
        rox_i18n::format::format_unit((20.0 * volume.log10()) as f64, 1, "dB")
    }
}

/// The step is shared with the playback strip's speaker button.
fn volume_scroll(
    this: &mut VolumePanel,
    event: &gpui::ScrollWheelEvent,
    _window: &mut Window,
    cx: &mut Context<VolumePanel>,
) {
    super::volume_wheel(&this.state.player, event, cx);
}

impl Render for VolumePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(cx).track_focus(&focus))
    }
}

impl VolumePanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let player = self.state.player.read(cx);
        let volume = player.volume();
        let muted = player.muted();
        let level = if self.config.percent_db {
            fmt_db(volume)
        } else {
            rox_i18n::format::format_percent((volume * 100.0).round() as f64)
        };

        let (speaker, speaker_color) = if muted {
            (icons::VOLUME_X, palette::text_faint())
        } else if volume <= 0.5 {
            (icons::VOLUME_1, palette::text())
        } else {
            (icons::VOLUME_2, palette::text())
        };

        // With the readout hidden, the tip carries the level.
        let tip = match (muted, self.config.items.contains(&VolumeItem::Percent)) {
            (true, true) => rox_i18n::t!("volume-tip-unmute").to_string(),
            (true, false) => {
                rox_i18n::t!("volume-tip-unmute-level", level = level.clone()).to_string()
            }
            (false, true) => rox_i18n::t!("volume-tip-mute").to_string(),
            (false, false) => {
                rox_i18n::t!("volume-tip-mute-level", level = level.clone()).to_string()
            }
        };
        let icon = panel::Tip::keyed("volume-icon", tip).apply(
            div()
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
                .child(svg().path(speaker).size(px(16.)).text_color(speaker_color)),
        );

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
                    // A hand-edited volume past 100% shows as full.
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

        let readout = div()
            // Scale with the app font and never wrap: a larger font overruns the
            // fixed width.
            .w(px(if self.config.percent_db { 64. } else { 40. }) * palette::row_scale())
            .flex_none()
            .whitespace_nowrap()
            .text_center()
            .text_color(palette::text_muted())
            .child(level);

        let mut icon = Some(icon);
        let mut slider = Some(slider);
        let mut readout = Some(readout);
        let pieces: Vec<AnyElement> = self
            .config
            .items
            .iter()
            .filter_map(|item| match item {
                VolumeItem::Icon => icon.take().map(|e| e.into_any_element()),
                VolumeItem::Slider => slider.take().map(|e| e.into_any_element()),
                VolumeItem::Percent => readout.take().map(|e| e.into_any_element()),
                VolumeItem::Spacer => Some(div().flex_1().into_any_element()),
            })
            .collect();

        div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .items_center()
            .map(|d| justify(d, self.config.align))
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_MD)
            .on_scroll_wheel(cx.listener(volume_scroll))
            .children(pieces)
    }
}

transport_panel!(
    VolumePanel,
    "volume",
    rox_i18n::t!("panel-title-volume"),
    min_w = |_: &VolumePanel| rox_dock::resizable::PANEL_MIN_SIZE
);

#[cfg(test)]
mod tests {
    use super::{VolumeConfig, VolumeItem};

    #[test]
    fn icon_only_folds_into_the_item_list() {
        let config: VolumeConfig = serde_json::from_str(r#"{"icon_only": true}"#).unwrap();
        assert!(config.items == vec![VolumeItem::Icon]);
    }

    #[test]
    fn missing_toggles_default_on() {
        let config: VolumeConfig = serde_json::from_str("{}").unwrap();
        assert!(config.items == VolumeConfig::default().items);
    }

    #[test]
    fn legacy_forms_fold_into_the_item_list() {
        let config: VolumeConfig =
            serde_json::from_str(r#"{"icon": false, "percent": true}"#).unwrap();
        assert!(config.items == vec![VolumeItem::Slider, VolumeItem::Percent]);

        let config: VolumeConfig =
            serde_json::from_str(r#"{"icon": "right", "percent": "hidden"}"#).unwrap();
        assert!(config.items == vec![VolumeItem::Slider, VolumeItem::Icon]);

        let config: VolumeConfig =
            serde_json::from_str(r#"{"icon": "right", "percent": "left"}"#).unwrap();
        assert!(config.items == vec![VolumeItem::Percent, VolumeItem::Slider, VolumeItem::Icon]);
    }

    #[test]
    fn item_lists_read_ordered_and_deduped() {
        let config: VolumeConfig =
            serde_json::from_str(r#"{"items": ["percent", "icon", "percent"]}"#).unwrap();
        assert!(config.items == vec![VolumeItem::Percent, VolumeItem::Icon]);

        let saved = serde_json::to_value(&config).unwrap();
        let back: VolumeConfig = serde_json::from_value(saved).unwrap();
        assert!(back.items == config.items);
    }
}
