//! The transport panels: playback controls, the track info readout, the
//! volume strip, and the seek strip. Each is a view over the shared player
//! entity. This module holds what they share.

mod playback;
// Crate-visible because the waveform's corner mark reuses the seek
// strip's live-mark look.
pub(crate) mod seek;
mod track_info;
mod volume;

pub use playback::{TransportConfig, TransportPanel};
pub use seek::{SeekConfig, SeekStripPanel};
pub use track_info::{InfoPiece, TrackInfoConfig, TrackInfoPanel};
pub use volume::{VolumeConfig, VolumePanel};

use rox_panel_kit::config::default_true;

use gpui::{App, Entity, ScrollDelta, ScrollWheelEvent};

use crate::player::Player;

/// One wheel notch over a volume control: a notch is 3 lines, which
/// steps 5%.
pub(crate) fn volume_wheel(player: &Entity<Player>, event: &ScrollWheelEvent, cx: &mut App) {
    let lines = match event.delta {
        ScrollDelta::Lines(lines) => lines.y,
        ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / 20.0,
    };
    player.update(cx, |player, cx| {
        let volume = (player.volume() + lines / 3.0 * 0.05).clamp(0.0, 1.0);
        player.set_volume(volume, cx);
    });
}

/// The Panel and focus plumbing shared by the transport panels. Each one
/// has a `config` field, a `config_menu` method, and a PanelSettings impl.
/// `min_w` is the width the layout won't squeeze the panel below; pass a
/// closure over `&self` when it depends on the config.
macro_rules! transport_panel {
    ($panel:ty, $name:literal, $title:expr, min_w = $min_w:literal) => {
        transport_panel!($panel, $name, $title, min_w = |_: &$panel| px($min_w));
    };
    ($panel:ty, $name:literal, $title:expr, min_w = $min_w:expr) => {
        transport_panel!(
            $panel,
            $name,
            $title,
            min_w = $min_w,
            min_h = |_: &$panel| rox_dock::resizable::PANEL_MIN_SIZE
        );
    };
    ($panel:ty, $name:literal, $title:expr, min_w = $min_w:expr, min_h = $min_h:expr) => {
        impl EventEmitter<PanelEvent> for $panel {}

        impl Focusable for $panel {
            fn focus_handle(&self, _cx: &App) -> FocusHandle {
                self.focus.clone()
            }
        }

        impl Panel for $panel {
            fn panel_name(&self) -> &'static str {
                $name
            }

            rox_panel_api::opens_settings!();

            fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
                panel::title_text(self.config.chrome.title.as_deref(), $title)
            }

            fn tab_name(&self, _cx: &App) -> Option<gpui::SharedString> {
                self.config
                    .chrome
                    .title
                    .clone()
                    .map(gpui::SharedString::from)
            }

            fn locked(&self, _cx: &App) -> bool {
                self.config.chrome.locked
            }

            fn inner_padding(&self, _cx: &App) -> bool {
                false
            }

            fn min_size(&self, _cx: &App) -> gpui::Size<Pixels> {
                crate::panel::chrome_min_size(
                    &self.config.chrome,
                    gpui::size(($min_w)(self), ($min_h)(self)),
                )
            }

            fn max_size(&self, cx: &App) -> gpui::Size<Pixels> {
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
                _window: &mut Window,
                cx: &mut Context<Self>,
            ) -> PopupMenu {
                let menu = self.config_menu(menu, _window, cx);
                let menu = panel_settings::rename_item(
                    menu,
                    &cx.entity(),
                    self.tab_panel.clone(),
                    _window,
                    cx,
                );
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
                        <$panel>::new(state, config, cx)
                    },
                );
                panel::popout_item(
                    menu,
                    &cx.entity(),
                    self.tab_panel.clone(),
                    self.state.clone(),
                    _window,
                )
            }
        }
    };
}
pub(crate) use transport_panel;
