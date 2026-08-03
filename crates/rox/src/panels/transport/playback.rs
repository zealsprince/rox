//! The playback controls panel: prev, the seek nudges around play/pause,
//! next, and the loop and shuffle modes, plus the optional stop and random
//! buttons.

use std::time::Instant;

use gpui::{
    div, prelude::*, svg, AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable,
    MouseButton, Pixels, Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_playback::engine::LoopMode;

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{self, align_row, justify, Align, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::player::observe_view;

use super::{default_true, transport_panel};

/// One button of the playback strip, the arrange editor's unit. The
/// config's list carries the shown ones in display order.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackItem {
    /// The previous-track button.
    Prev,
    /// The ten-second back nudge.
    SeekBack,
    /// The play/pause button, the primary transport action.
    Play,
    /// The ten-second forward nudge.
    SeekForward,
    /// The next-track button.
    Next,
    /// The stop button that ejects the playing track.
    Stop,
    /// The loop button that cycles off, all, one.
    Repeat,
    /// The shuffle button.
    Shuffle,
    /// The random button that plays one track from anywhere in the library.
    Random,
    /// The stop-after-current toggle: armed, the playing track ends the
    /// motion and the next one cues up paused.
    StopAfter,
    /// A flexible gap that pushes the buttons around it apart. One per
    /// strip under the unique-item model.
    Spacer,
}

/// The strip's full catalog in stock order: what the arrange editor
/// offers, and where a menu toggle slots a re-shown button back in.
const ITEMS: &[panel::ArrangeSpec<PlaybackItem>] = &[
    panel::ArrangeSpec {
        label: "Previous",
        icon: Some(icons::SKIP_BACK),
        value: PlaybackItem::Prev,
    },
    panel::ArrangeSpec {
        label: "Seek Back",
        icon: Some(icons::REWIND),
        value: PlaybackItem::SeekBack,
    },
    panel::ArrangeSpec {
        label: "Play",
        icon: Some(icons::PLAY),
        value: PlaybackItem::Play,
    },
    panel::ArrangeSpec {
        label: "Seek Forward",
        icon: Some(icons::FAST_FORWARD),
        value: PlaybackItem::SeekForward,
    },
    panel::ArrangeSpec {
        label: "Next",
        icon: Some(icons::SKIP_FORWARD),
        value: PlaybackItem::Next,
    },
    panel::ArrangeSpec {
        label: "Stop",
        icon: Some(icons::STOP),
        value: PlaybackItem::Stop,
    },
    panel::ArrangeSpec {
        label: "Loop",
        icon: Some(icons::REPEAT),
        value: PlaybackItem::Repeat,
    },
    panel::ArrangeSpec {
        label: "Shuffle",
        icon: Some(icons::SHUFFLE),
        value: PlaybackItem::Shuffle,
    },
    panel::ArrangeSpec {
        label: "Random",
        icon: Some(icons::DICE),
        value: PlaybackItem::Random,
    },
    panel::ArrangeSpec {
        label: "Stop After",
        icon: Some(icons::SQUARE_DASHED),
        value: PlaybackItem::StopAfter,
    },
    panel::ArrangeSpec {
        label: "Spacer",
        icon: Some(icons::MOVE_HORIZONTAL),
        value: PlaybackItem::Spacer,
    },
];

/// The playback panel's per-view config: what a saved layout restores,
/// and what the settings window edits. Deserialization routes through
/// [`TransportConfigDump`] so layouts from before the buttons became an
/// ordered list still read.
#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "TransportConfigDump")]
pub struct TransportConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub align: Align,
    /// The play button's accent highlight shape, or none for a flat
    /// button like the rest of the strip.
    pub play_highlight: PlayHighlight,
    /// The shown buttons in display order; one not listed is hidden.
    pub items: Vec<PlaybackItem>,
}

impl Default for TransportConfig {
    fn default() -> Self {
        // Everything but stop and random ships on, in the order the strip
        // always rendered: nudges around play, the modes trailing.
        TransportConfig {
            chrome: PanelChrome::default(),
            align: Align::default(),
            play_highlight: PlayHighlight::default(),
            items: vec![
                PlaybackItem::Prev,
                PlaybackItem::SeekBack,
                PlaybackItem::Play,
                PlaybackItem::SeekForward,
                PlaybackItem::Next,
                PlaybackItem::Repeat,
                PlaybackItem::Shuffle,
            ],
        }
    }
}

/// The dump shape [`TransportConfig`] deserializes through: the ordered
/// list newer layouts write, or the per-button toggles older ones carried,
/// folded back in the order the strip used to render. The one `seek`
/// toggle was both nudges around play.
#[derive(Deserialize)]
struct TransportConfigDump {
    #[serde(flatten)]
    chrome: PanelChrome,
    #[serde(default)]
    align: Align,
    #[serde(default)]
    play_highlight: PlayHighlight,
    #[serde(default)]
    items: Option<Vec<PlaybackItem>>,
    #[serde(default = "default_true")]
    prev: bool,
    #[serde(default = "default_true")]
    play: bool,
    #[serde(default = "default_true")]
    next: bool,
    #[serde(default = "default_true")]
    seek: bool,
    #[serde(default = "default_true")]
    repeat: bool,
    #[serde(default = "default_true")]
    shuffle: bool,
    #[serde(default)]
    stop: bool,
    #[serde(default)]
    random: bool,
}

impl From<TransportConfigDump> for TransportConfig {
    fn from(dump: TransportConfigDump) -> Self {
        let items = match dump.items {
            Some(items) => panel::dedup(items),
            None => {
                let mut items = Vec::new();
                let mut on = |on, item| {
                    if on {
                        items.push(item)
                    }
                };
                on(dump.prev, PlaybackItem::Prev);
                on(dump.seek, PlaybackItem::SeekBack);
                on(dump.play, PlaybackItem::Play);
                on(dump.seek, PlaybackItem::SeekForward);
                on(dump.next, PlaybackItem::Next);
                on(dump.stop, PlaybackItem::Stop);
                on(dump.repeat, PlaybackItem::Repeat);
                on(dump.shuffle, PlaybackItem::Shuffle);
                on(dump.random, PlaybackItem::Random);
                items
            }
        };
        TransportConfig {
            chrome: dump.chrome,
            align: dump.align,
            play_highlight: dump.play_highlight,
            items,
        }
    }
}

/// The play button's accent highlight: the filled disc it ships with, a
/// soft square on the shared control radius, or no fill at all so the
/// button sits flat like its neighbors.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayHighlight {
    /// The filled disc, fully round.
    #[default]
    Circle,
    /// A soft square, the same radius every other control wears.
    Square,
    /// No highlight, a flat icon like the other buttons.
    None,
}

/// The playback controls: prev, the seek nudges around play/pause, next,
/// and the loop and shuffle modes, plus the optional stop and random
/// buttons. What is
/// playing lives in the track info panel. The pump's
/// tick notifies the player while a session runs, so the observe below
/// keeps the play state fresh even in a popped-out window.
pub struct TransportPanel {
    state: AppState,
    config: TransportConfig,
    focus: FocusHandle,
    /// The tab panel this panel currently sits in, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// The last crossfade the render saw, so the frame where it disappears
    /// can tell a finished fade (glow out) from a cancelled one (vanish,
    /// today's behavior).
    last_fade: Option<crate::player::FadeView>,
    /// A finished fade's afterglow: when it landed and which button wore
    /// the sweep. The gated observer goes quiet the moment the fade ends,
    /// so the render drives these frames itself.
    outro: Option<(Instant, bool)>,
    _player_changed: Subscription,
}

/// A fade that got at least this far before disappearing finished; anything
/// earlier was cancelled by a stop or a seek and shouldn't celebrate. Short
/// of 1.0 because the observer wakes per quantized step and the last step
/// may never be seen.
const OUTRO_FROM: f32 = 0.85;

impl TransportPanel {
    pub fn new(state: AppState, config: TransportConfig, cx: &mut Context<Self>) -> Self {
        // Play state, loop, and shuffle change on a user action, never on
        // the position tick, so ride the gated observe.
        let _player_changed = observe_view(&state.player, cx);
        TransportPanel {
            state,
            config,
            focus: cx.focus_handle(),
            tab_panel: None,
            last_fade: None,
            outro: None,
            _player_changed,
        }
    }

    /// The panel's own dropdown entries: quick show/hide for the opt-in
    /// buttons. A re-shown one slots back at its stock position; the
    /// settings window's arrange editor is where the order changes.
    fn config_menu(&self, menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        let mut menu = menu;
        for (name, value) in [
            ("Stop Button", PlaybackItem::Stop),
            ("Random Button", PlaybackItem::Random),
            ("Stop After Button", PlaybackItem::StopAfter),
        ] {
            let weak = cx.entity().downgrade();
            menu = menu.item(
                PopupMenuItem::new(name)
                    .checked(self.config.items.contains(&value))
                    .on_click(move |_, _, cx| {
                        let Some(this) = weak.upgrade() else { return };
                        this.update(cx, |this, cx| {
                            this.config.items = panel::toggled(ITEMS, &this.config.items, value);
                            cx.notify();
                        });
                    }),
            );
        }
        menu
    }

    /// Pick one track from anywhere in the library and play it as a fresh
    /// one-track queue.
    fn play_random(&mut self, cx: &mut Context<Self>) {
        let paths = {
            let library = self.state.library.read(cx);
            let Some(projection) = library.projection() else {
                return;
            };
            if projection.is_empty() {
                return;
            }
            let id = projection.db_id[random_index(projection.len())];
            library.paths_for(&[id]).ok()
        };
        let Some(paths) = paths else { return };
        self.state
            .player
            .update(cx, |player, cx| player.play(paths, cx));
    }
}

/// A random index below `len`, off the std hasher's per-process random
/// keys; picking a track does not need a rand dependency.
fn random_index(len: usize) -> usize {
    use std::hash::{BuildHasher, Hasher};
    let hash = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    (hash % len as u64) as usize
}

impl PanelSettings for TransportPanel {
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
        &[("Controls", icons::PLAY)]
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
                "Buttons",
                Some(
                    "Drag along the bar to reorder; drag between the rows, \
                     or use a chip's x and plus, to hide and show",
                ),
                None,
                panel::arrange_editor(
                    "playback-items",
                    ITEMS,
                    &self.config.items,
                    |this: &mut Self, items, cx| {
                        this.config.items = items;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(self.config.items.contains(&PlaybackItem::Play), |d| {
                d.child(panel::setting_row(
                    "Play Highlight",
                    Some("The play button's accent fill: a circle, a soft square, or none"),
                    panel::choices(
                        &[
                            ("Circle", PlayHighlight::Circle),
                            ("Square", PlayHighlight::Square),
                            ("None", PlayHighlight::None),
                        ],
                        self.config.play_highlight,
                        |this: &mut Self, highlight, cx| {
                            this.config.play_highlight = highlight;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .into_any_element()
    }
}

impl Render for TransportPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        let body = panel::themed(&chrome, || self.body(cx));
        // The afterglow runs after the fade the observer was watching is
        // gone, so nothing else wakes this panel; it asks for its own
        // frames until the glow lands at zero.
        if self.outro.is_some() {
            window.request_animation_frame();
        }
        body
    }
}

impl TransportPanel {
    fn body(&mut self, cx: &mut Context<Self>) -> Div {
        let player = self.state.player.read(cx);
        let playing = player.is_playing();
        let active = player.is_active();
        // Loop state reads through the button itself: dim while off, the
        // accent while on, the one-track glyph for single-track loop.
        let (loop_icon, loop_color) = match player.loop_mode() {
            LoopMode::Off => (icons::REPEAT, palette::text_faint()),
            LoopMode::All => (icons::REPEAT, palette::accent()),
            LoopMode::One => (icons::REPEAT_1, palette::accent()),
        };
        // Shuffle reads the same way: dim while off, the accent while on.
        let shuffle_color = if player.shuffle() {
            palette::accent()
        } else {
            palette::text_faint()
        };
        // Stop-after too: dim until armed, the accent while it waits.
        let stop_after_color = if player.stop_after() {
            palette::accent()
        } else {
            palette::text_faint()
        };
        // A crossfade in flight sweeps across the button that started it,
        // so the overlap the ear is hearing is visible and reads in the
        // direction the queue moved. A boundary fade shows on Next, the way
        // the queue went.
        let fade = player.crossfade();
        // The frame where the fade disappears decides its exit. Finished
        // means the afterglow below; cancelled means gone, since glowing
        // over a stop would congratulate an interruption.
        if fade.is_some() {
            self.outro = None;
            self.last_fade = fade;
        } else if let Some(last) = self.last_fade.take() {
            if last.progress() >= OUTRO_FROM {
                self.outro = Some((Instant::now(), last.back));
            }
        }
        // The afterglow's strength this frame: the flash lands at full and
        // the square falls it away, most of the dissolve in the front half.
        let outro = self.outro.and_then(|(at, back)| {
            let t = at.elapsed().as_secs_f32() / tokens::EASE_SECS;
            (t < 1.0).then_some((back, (1.0 - t) * (1.0 - t)))
        });
        if outro.is_none() {
            self.outro = None;
        }

        // The strip renders the config's list as-is: each shown button in
        // its place, whatever order the arrange editor left them in.
        let highlight = self.config.play_highlight;
        let mut controls: Vec<AnyElement> = Vec::new();
        for item in self.config.items.clone() {
            controls.push(match item {
                PlaybackItem::Prev => panel::icon_control_fading(
                    icons::SKIP_BACK,
                    palette::text(),
                    fade.filter(|fade| fade.back),
                    outro
                        .filter(|(back, _)| *back)
                        .map(|(_, strength)| strength),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.prev()),
                    cx,
                )
                .into_any_element(),
                PlaybackItem::SeekBack => panel::icon_control(
                    icons::REWIND,
                    palette::text(),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.seek_by(-10.0)),
                    cx,
                )
                .into_any_element(),
                // Play/pause is the primary action, so it gets the accent
                // fill while everything around it stays flat; the config
                // picks the fill's shape, or drops it to match the
                // neighbors.
                PlaybackItem::Play => div()
                    .size(tokens::PLAY_SIZE)
                    .flex_none()
                    .map(|d| match highlight {
                        PlayHighlight::Circle => d
                            .rounded_full()
                            .bg(palette::accent())
                            .hover(|d| d.bg(palette::accent_hover())),
                        PlayHighlight::Square => d
                            .rounded(tokens::RADIUS)
                            .bg(palette::accent())
                            .hover(|d| d.bg(palette::accent_hover())),
                        PlayHighlight::None => d
                            .rounded(tokens::RADIUS)
                            .hover(|d| d.bg(palette::bg_control())),
                    })
                    .cursor_pointer()
                    .flex()
                    .items_center()
                    .justify_center()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this: &mut Self, _, _, cx| {
                            this.state.player.update(cx, |p, _| p.toggle_pause())
                        }),
                    )
                    .child(
                        svg()
                            .path(if playing { icons::PAUSE } else { icons::PLAY })
                            .size_4()
                            .text_color(if highlight == PlayHighlight::None {
                                palette::text()
                            } else {
                                palette::text_on_accent()
                            }),
                    )
                    .into_any_element(),
                PlaybackItem::SeekForward => panel::icon_control(
                    icons::FAST_FORWARD,
                    palette::text(),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.seek_by(10.0)),
                    cx,
                )
                .into_any_element(),
                PlaybackItem::Next => panel::icon_control_fading(
                    icons::SKIP_FORWARD,
                    palette::text(),
                    fade.filter(|fade| !fade.back),
                    outro
                        .filter(|(back, _)| !*back)
                        .map(|(_, strength)| strength),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.next()),
                    cx,
                )
                .into_any_element(),
                // Stop ejects the track: the session drops and every view
                // over it goes idle. Dim while nothing is loaded.
                PlaybackItem::Stop => panel::icon_control(
                    icons::STOP,
                    if active {
                        palette::text()
                    } else {
                        palette::text_faint()
                    },
                    |this: &mut Self, cx| this.state.player.update(cx, |p, cx| p.stop(cx)),
                    cx,
                )
                .into_any_element(),
                PlaybackItem::Repeat => panel::icon_control(
                    loop_icon,
                    loop_color,
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.cycle_loop()),
                    cx,
                )
                .into_any_element(),
                PlaybackItem::Shuffle => panel::icon_control(
                    icons::SHUFFLE,
                    shuffle_color,
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.toggle_shuffle()),
                    cx,
                )
                .into_any_element(),
                PlaybackItem::Random => panel::icon_control(
                    icons::DICE,
                    palette::text(),
                    |this: &mut Self, cx| this.play_random(cx),
                    cx,
                )
                .into_any_element(),
                PlaybackItem::StopAfter => panel::icon_control(
                    icons::SQUARE_DASHED,
                    stop_after_color,
                    |this: &mut Self, cx| {
                        this.state
                            .player
                            .update(cx, |p, cx| p.toggle_stop_after(cx))
                    },
                    cx,
                )
                .into_any_element(),
                PlaybackItem::Spacer => div().flex_1().into_any_element(),
            });
        }

        div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .items_center()
            .map(|d| justify(d, self.config.align))
            .gap(tokens::SPACE_XS)
            .px(tokens::SPACE_SM)
            .children(controls)
    }
}

// The playback row is fully composable, so it leans on the app's own panel
// floor instead of pinning a width.
transport_panel!(
    TransportPanel,
    "playback",
    "Playback",
    min_w = |_: &TransportPanel| rox_dock::resizable::PANEL_MIN_SIZE
);

#[cfg(test)]
mod tests {
    use super::{PlaybackItem, TransportConfig};

    /// A layout with no button fields at all decodes to the stock strip:
    /// nudges around play, the modes trailing, stop and random off.
    #[test]
    fn missing_toggles_default_to_the_stock_strip() {
        let config: TransportConfig = serde_json::from_str("{}").unwrap();
        assert!(config.items == TransportConfig::default().items);
    }

    /// The per-button toggles older layouts wrote fold into the list in
    /// the order the strip used to render; the one seek toggle was both
    /// nudges.
    #[test]
    fn legacy_toggles_fold_in_render_order() {
        let config: TransportConfig =
            serde_json::from_str(r#"{"seek": false, "shuffle": false, "stop": true}"#).unwrap();
        assert!(
            config.items
                == vec![
                    PlaybackItem::Prev,
                    PlaybackItem::Play,
                    PlaybackItem::Next,
                    PlaybackItem::Stop,
                    PlaybackItem::Repeat,
                ]
        );
    }

    /// A layout that carries the list uses it as-is, duplicates dropped,
    /// and round-trips through a save.
    #[test]
    fn item_lists_read_ordered_and_deduped() {
        let config: TransportConfig =
            serde_json::from_str(r#"{"items": ["shuffle", "play", "shuffle"]}"#).unwrap();
        assert!(config.items == vec![PlaybackItem::Shuffle, PlaybackItem::Play]);

        let saved = serde_json::to_value(&config).unwrap();
        let back: TransportConfig = serde_json::from_value(saved).unwrap();
        assert!(back.items == config.items);
    }
}
