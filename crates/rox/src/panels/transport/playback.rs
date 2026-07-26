//! The playback controls panel: prev, the seek nudges around play/pause,
//! next, and the loop and shuffle modes, plus the optional stop and random
//! buttons.

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

/// The playback panel's per-view config: what a saved layout restores,
/// and what the settings window edits.
#[derive(Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    #[serde(default)]
    pub align: Align,
    /// The previous-track button.
    #[serde(default = "default_true")]
    pub prev: bool,
    /// The play/pause button, the primary transport action.
    #[serde(default = "default_true")]
    pub play: bool,
    /// The play button's accent highlight shape, or none for a flat
    /// button like the rest of the strip.
    #[serde(default)]
    pub play_highlight: PlayHighlight,
    /// The next-track button.
    #[serde(default = "default_true")]
    pub next: bool,
    /// The seek nudge buttons that jump back and forward ten seconds. On by
    /// default; drop them on a compact bar that only needs prev/play/next.
    #[serde(default = "default_true")]
    pub seek: bool,
    /// The loop button that cycles off, all, one.
    #[serde(default = "default_true")]
    pub repeat: bool,
    /// The shuffle button.
    #[serde(default = "default_true")]
    pub shuffle: bool,
    /// The stop button that ejects the playing track.
    #[serde(default)]
    pub stop: bool,
    /// The random button that plays one track from anywhere in the library.
    #[serde(default)]
    pub random: bool,
}

impl Default for TransportConfig {
    fn default() -> Self {
        // The seek/loop/shuffle buttons ship on, matching what a layout with
        // none of these fields set decodes to; only stop and random are
        // opt-in.
        TransportConfig {
            chrome: PanelChrome::default(),
            align: Align::default(),
            prev: true,
            play: true,
            play_highlight: PlayHighlight::default(),
            next: true,
            seek: true,
            repeat: true,
            shuffle: true,
            stop: false,
            random: false,
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
    _player_changed: Subscription,
}

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
            _player_changed,
        }
    }

    /// The panel's own dropdown entries: the optional button toggles, the
    /// same knobs the customize window edits.
    fn config_menu(&self, menu: PopupMenu, cx: &mut Context<Self>) -> PopupMenu {
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Stop Button")
                .checked(self.config.stop)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.stop = !this.config.stop;
                        cx.notify();
                    });
                }),
        );
        let weak = cx.entity().downgrade();
        menu.item(
            PopupMenuItem::new("Random Button")
                .checked(self.config.random)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.random = !this.config.random;
                        cx.notify();
                    });
                }),
        )
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
            .child(panel::setting_row(
                "Previous",
                Some("The previous-track button"),
                panel::toggle(
                    self.config.prev,
                    |this: &mut Self, prev, cx| {
                        this.config.prev = prev;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Play",
                Some("The play and pause button"),
                panel::toggle(
                    self.config.play,
                    |this: &mut Self, play, cx| {
                        this.config.play = play;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(self.config.play, |d| {
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
            .child(panel::setting_row(
                "Next",
                Some("The next-track button"),
                panel::toggle(
                    self.config.next,
                    |this: &mut Self, next, cx| {
                        this.config.next = next;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Seek Buttons",
                Some("The back and forward ten-second nudges around play"),
                panel::toggle(
                    self.config.seek,
                    |this: &mut Self, seek, cx| {
                        this.config.seek = seek;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Loop",
                Some("The loop button that cycles off, all, one"),
                panel::toggle(
                    self.config.repeat,
                    |this: &mut Self, repeat, cx| {
                        this.config.repeat = repeat;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Shuffle",
                Some("The shuffle button"),
                panel::toggle(
                    self.config.shuffle,
                    |this: &mut Self, shuffle, cx| {
                        this.config.shuffle = shuffle;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Stop",
                Some("The stop button that ejects the playing track"),
                panel::toggle(
                    self.config.stop,
                    |this: &mut Self, stop, cx| {
                        this.config.stop = stop;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Random",
                Some("The random button that plays one track from anywhere in the library"),
                panel::toggle(
                    self.config.random,
                    |this: &mut Self, random, cx| {
                        this.config.random = random;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .into_any_element()
    }
}

impl Render for TransportPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(cx))
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

        // Play/pause is the primary action, so it gets the accent fill
        // while everything around it stays flat; the config picks the
        // fill's shape, or drops it to match the neighbors.
        let highlight = self.config.play_highlight;
        let play_pause = div()
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
            );

        div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .items_center()
            .map(|d| justify(d, self.config.align))
            .gap(tokens::SPACE_XS)
            .px(tokens::SPACE_SM)
            .when(self.config.prev, |d| {
                d.child(panel::icon_control(
                    icons::SKIP_BACK,
                    palette::text(),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.prev()),
                    cx,
                ))
            })
            .when(self.config.seek, |d| {
                d.child(panel::icon_control(
                    icons::REWIND,
                    palette::text(),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.seek_by(-10.0)),
                    cx,
                ))
            })
            .when(self.config.play, |d| d.child(play_pause))
            .when(self.config.seek, |d| {
                d.child(panel::icon_control(
                    icons::FAST_FORWARD,
                    palette::text(),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.seek_by(10.0)),
                    cx,
                ))
            })
            .when(self.config.next, |d| {
                d.child(panel::icon_control(
                    icons::SKIP_FORWARD,
                    palette::text(),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.next()),
                    cx,
                ))
            })
            // Stop ejects the track: the session drops and every view over
            // it goes idle. Dim while nothing is loaded.
            .when(self.config.stop, |d| {
                d.child(panel::icon_control(
                    icons::STOP,
                    if active {
                        palette::text()
                    } else {
                        palette::text_faint()
                    },
                    |this: &mut Self, cx| this.state.player.update(cx, |p, cx| p.stop(cx)),
                    cx,
                ))
            })
            .when(self.config.repeat, |d| {
                d.child(panel::icon_control(
                    loop_icon,
                    loop_color,
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.cycle_loop()),
                    cx,
                ))
            })
            .when(self.config.shuffle, |d| {
                d.child(panel::icon_control(
                    icons::SHUFFLE,
                    shuffle_color,
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.toggle_shuffle()),
                    cx,
                ))
            })
            .when(self.config.random, |d| {
                d.child(panel::icon_control(
                    icons::DICE,
                    palette::text(),
                    |this: &mut Self, cx| this.play_random(cx),
                    cx,
                ))
            })
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
