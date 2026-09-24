//! The playback controls panel: prev, the seek nudges around play/pause,
//! next, and the loop and shuffle modes, plus the opt-in buttons the
//! arrange editor offers.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Context, DismissEvent, Div, Entity, EventEmitter, FocusHandle, Focusable,
    MouseButton, Pixels, Point, Stateful, Subscription, WeakEntity, Window, anchored, canvas,
    deferred, div, prelude::*, px, svg,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::spinner::Spinner;
use gpui_component::{Icon, Side};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::TrackKey;
use serde::{Deserialize, Serialize};

use rox_playback::StreamState;
use rox_playback::engine::LoopMode;

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::continuation;
use crate::design::{palette, tokens};
use crate::panel::{
    self, Align, AppState, PanelChrome, PanelSettings, ScrubState, align_row, justify,
};
use crate::panel_settings;
use crate::player::{AbState, fmt_time, observe_view};
use crate::rating_ui;
use crate::settings::ShuffleMode;
use crate::source::TrackSource;
use rox_panel_api::actions::{PLAYBACK_TIP_SCOPE, TogglePlayback};

use super::{default_true, transport_panel};

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackItem {
    Prev,
    SeekBack,
    Play,
    SeekForward,
    Next,
    Stop,
    /// The speaker button: a click mutes, the wheel nudges the level, and a
    /// right-click opens the slider. The volume strip without the strip,
    /// for a transport that only has room for the one button.
    Volume,
    Repeat,
    Shuffle,
    /// Whether a queue that runs out keeps playing (ADR 17).
    Continue,
    /// Whether track boundaries overlap, and for how long (ADR 19).
    Crossfade,
    Random,
    /// Armed, the playing track ends the motion and the next cues up paused.
    StopAfter,
    /// Mark A, mark B, and a third press clears the repeat.
    AbRepeat,
    Favourite,
    Rating,
    /// A flexible gap; the strip holds as many as the layout needs.
    Spacer,
}

/// Stock order: where a menu toggle slots a re-shown button back in.
const ITEMS: &[panel::ArrangeSpec<PlaybackItem>] = &[
    panel::ArrangeSpec {
        key: "playback-item-previous",
        icon: Some(icons::SKIP_BACK),
        value: PlaybackItem::Prev,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "playback-item-seek-back",
        icon: Some(icons::REWIND),
        value: PlaybackItem::SeekBack,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "playback-item-play",
        icon: Some(icons::PLAY),
        value: PlaybackItem::Play,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "playback-item-seek-forward",
        icon: Some(icons::FAST_FORWARD),
        value: PlaybackItem::SeekForward,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "playback-item-next",
        icon: Some(icons::SKIP_FORWARD),
        value: PlaybackItem::Next,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "playback-item-stop",
        icon: Some(icons::STOP),
        value: PlaybackItem::Stop,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "playback-item-volume",
        icon: Some(icons::VOLUME_2),
        value: PlaybackItem::Volume,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "playback-item-loop",
        icon: Some(icons::REPEAT),
        value: PlaybackItem::Repeat,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "playback-item-shuffle",
        icon: Some(icons::SHUFFLE),
        value: PlaybackItem::Shuffle,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "playback-item-continue",
        icon: Some(icons::INFINITY),
        value: PlaybackItem::Continue,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "playback-item-crossfade",
        icon: Some(icons::BLEND),
        value: PlaybackItem::Crossfade,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "playback-item-random",
        icon: Some(icons::DICE),
        value: PlaybackItem::Random,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "playback-item-stop-after",
        icon: Some(icons::SQUARE_DASHED),
        value: PlaybackItem::StopAfter,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "playback-item-ab-repeat",
        icon: Some(icons::MOVE_HORIZONTAL),
        value: PlaybackItem::AbRepeat,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "playback-item-favourite",
        icon: Some(icons::HEART),
        value: PlaybackItem::Favourite,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "playback-item-rating",
        icon: Some(icons::STAR),
        value: PlaybackItem::Rating,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "head-piece-spacer",
        icon: Some(icons::MOVE_HORIZONTAL),
        value: PlaybackItem::Spacer,
        repeats: true,
    },
];

/// Reads through [`TransportConfigDump`] so layouts from before the
/// ordered list still load.
#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "TransportConfigDump")]
pub struct TransportConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub align: Align,
    pub play_highlight: PlayHighlight,
    /// Display order; one not listed is hidden.
    pub items: Vec<PlaybackItem>,
    pub random_mode: RandomMode,
}

impl Default for TransportConfig {
    fn default() -> Self {
        // Continue is opt-in even though continuation ships on (ADR 17): its
        // strategy is picked on the Behavior page, and a stock button for every
        // quiet mode turns a transport into a dashboard.
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
            random_mode: RandomMode::default(),
        }
    }
}

/// Newer layouts write the ordered list. Older ones had per-button
/// toggles, folded back in the order the strip used to render; the one
/// `seek` toggle was both nudges.
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
    #[serde(default)]
    random_mode: RandomMode,
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
            Some(items) => panel::dedup(ITEMS, items),
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
                // Continue, crossfade, volume and the heart aren't in the stock strip,
                // so an old layout comes back looking like a fresh install.
                on(dump.random, PlaybackItem::Random);
                items
            }
        };
        TransportConfig {
            chrome: dump.chrome,
            align: dump.align,
            play_highlight: dump.play_highlight,
            items,
            random_mode: dump.random_mode,
        }
    }
}

/// A layout from before the dropdown reads as Random, the only draw it
/// ever did.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RandomMode {
    #[default]
    Random,
    /// Off the acoustic vectors; needs the library analyzed.
    Similar,
}

impl RandomMode {
    fn label(self) -> &'static str {
        match self {
            RandomMode::Random => rox_i18n::t_static("playback-item-random"),
            RandomMode::Similar => rox_i18n::t_static("library-play-similar"),
        }
    }

    const ALL: [RandomMode; 2] = [RandomMode::Random, RandomMode::Similar];
}

#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayHighlight {
    #[default]
    Circle,
    /// The same radius every other control uses.
    Square,
    None,
}

/// What's playing belongs to the track info panel.
pub struct TransportPanel {
    state: AppState,
    config: TransportConfig,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// The row before a menu toggle hid a control, so re-showing it restores
    /// its place. Panel state, not config.
    items_stash: Option<Vec<PlaybackItem>>,
    /// Lets the frame where the fade disappears tell a finished fade (glow
    /// out) from a cancelled one (vanish).
    last_fade: Option<crate::player::FadeView>,
    /// When a finished fade ended and which button drew it. The gated
    /// observer goes quiet at the end, so the render drives these frames.
    outro: Option<(Instant, bool)>,
    /// When the A-B cycle took its first mark. Nothing else wakes the panel
    /// for the pulse, so the render drives it.
    ab_waiting: Option<Instant>,
    /// Bumped per mode press, so a stale hold check knows its press is over.
    press_seq: u64,
    /// Taken on release, so a press that turned into a hold doesn't also
    /// toggle.
    press: Option<ModePress>,
    /// The `PopoutHost` dock menu's shape: a gpui-component context menu only
    /// opens on right-click.
    mode_menu: Option<(Point<Pixels>, Entity<PopupMenu>, Subscription)>,
    /// Where the right-click on the speaker hit, while the slider is open.
    volume_at: Option<Point<Pixels>>,
    volume_scrub: ScrubState,
    /// Cached so a frame never turns into a database lookup.
    heart: Option<Heart>,
    _player_changed: Subscription,
    _library_changed: Subscription,
}

/// The key it was resolved from, its catalog id (None for a file the
/// library doesn't know), its favourite state, and its rating.
struct Heart {
    key: TrackKey,
    id: Option<i64>,
    on: bool,
    rating: u8,
}

/// A click does something and a hold opens its shades: shuffle order,
/// crossfade length, and the draw. Continue isn't one: its strategies
/// differ in kind and need a sentence each on the Behavior page.
#[derive(Clone, Copy, PartialEq)]
enum ModeButton {
    Shuffle,
    Crossfade,
    Random,
}

struct ModePress {
    /// The delayed hold check compares against the panel's counter, so a
    /// replaced press can't open a menu for the one after it.
    seq: u64,
    button: ModeButton,
    /// Set by the delayed check, so the release knows not to toggle.
    opened: bool,
}

/// Random keeps the crossed arrows. Similar takes the radio, the metaphor
/// people already have for "more of this".
fn mode_icon(mode: ShuffleMode) -> &'static str {
    match mode {
        ShuffleMode::Random => icons::SHUFFLE,
        ShuffleMode::Similar => icons::RADIO,
    }
}

/// Similar takes the waveform, not the shuffle's radio: two buttons in one
/// strip with the same glyph would read as the same switch twice.
fn draw_icon(mode: RandomMode) -> &'static str {
    match mode {
        RandomMode::Random => icons::DICE,
        RandomMode::Similar => icons::AUDIO_WAVEFORM,
    }
}

/// Zero is off. Round numbers only: the Audio page's scrub sets anything
/// between.
const CROSSFADE_LENGTHS: [f32; 5] = [0.0, 2.0, 4.0, 6.0, 10.0];

/// The scrub writes tenths, so equal at the readout's resolution.
fn is_length(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.05
}

/// Whole seconds, or a tenth where the scrub left it between.
fn length_label(secs: f32) -> String {
    if secs <= 0.0 {
        rox_i18n::t!("panel-size-off").to_string()
    } else if is_length(secs, secs.round()) {
        format!("{} s", rox_i18n::format::format_int(secs.round() as i64))
    } else {
        format!("{} s", rox_i18n::format::format_float(f64::from(secs), 1))
    }
}

/// Long enough that a click never hits it, short enough that the hold
/// doesn't feel broken.
const SHUFFLE_HOLD: Duration = Duration::from_millis(350);

/// Short of the volume strip's cap: this hangs over the transport and only
/// has to be long enough to aim at.
const VOLUME_POP_W: Pixels = px(120.);

/// A fade that got this far before disappearing finished; earlier was a
/// stop or a seek. Short of 1.0 because the observer wakes per quantized
/// step and may miss the last one.
const OUTRO_FROM: f32 = 0.85;

/// One breath of the A-B waiting dot.
const AB_PULSE_SECS: f32 = 1.6;
/// Never out: a dot that vanishes reads as the mark being dropped.
const AB_PULSE_FLOOR: f32 = 0.35;
const AB_DOT: Pixels = px(5.);

/// Window-sized so any press off the flyout hits it and closes it.
/// `size_full` would only span this short strip, leaving the flyout stuck
/// open after a click elsewhere in the app.
fn overlay_layer(window: &Window) -> Div {
    div()
        .w(window.bounds().size.width)
        .h(window.bounds().size.height)
        .occlude()
}

impl TransportPanel {
    pub fn new(state: AppState, config: TransportConfig, cx: &mut Context<Self>) -> Self {
        // Play state, loop and shuffle change on a user action, never on the
        // position tick, so the gated observe does.
        let _player_changed = observe_view(&state.player, cx);
        // Any playlist change moves the heart. A rescan can remap ids to paths,
        // so it drops the cache.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| match event {
                LibraryEvent::PlaylistsChanged => this.refresh_favourite(cx),
                LibraryEvent::Rated => this.refresh_rating(cx),
                LibraryEvent::Updated => {
                    this.heart = None;
                    cx.notify();
                }
                _ => {}
            },
        );
        TransportPanel {
            state,
            config,
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            items_stash: None,
            last_fade: None,
            outro: None,
            ab_waiting: None,
            press: None,
            mode_menu: None,
            volume_at: None,
            volume_scrub: ScrubState::default(),
            heart: None,
            press_seq: 0,
            _player_changed,
            _library_changed,
        }
    }

    /// A re-shown button goes back where it was; order changes in the
    /// arrange editor.
    fn config_menu(
        &self,
        menu: PopupMenu,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let mut menu = menu;
        for (name, value) in [
            (rox_i18n::t!("playback-menu-stop"), PlaybackItem::Stop),
            (rox_i18n::t!("playback-menu-volume"), PlaybackItem::Volume),
            (
                rox_i18n::t!("playback-menu-continue"),
                PlaybackItem::Continue,
            ),
            (
                rox_i18n::t!("playback-menu-crossfade"),
                PlaybackItem::Crossfade,
            ),
            (rox_i18n::t!("playback-menu-random"), PlaybackItem::Random),
            (
                rox_i18n::t!("playback-menu-stop-after"),
                PlaybackItem::StopAfter,
            ),
            (
                rox_i18n::t!("playback-menu-ab-repeat"),
                PlaybackItem::AbRepeat,
            ),
            (
                rox_i18n::t!("playback-menu-favourite"),
                PlaybackItem::Favourite,
            ),
            (rox_i18n::t!("playback-menu-rating"), PlaybackItem::Rating),
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
        menu
    }

    /// Its own control rather than [`panel::icon_control`]: that one fires on
    /// mouse down, and a hold has to swallow the click it started. The corner
    /// arrow also needs a positioned child.
    fn mode_control(
        &self,
        button: ModeButton,
        icon: &'static str,
        color: gpui::Rgba,
        tip: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let key = match button {
            ModeButton::Shuffle => "shuffle",
            ModeButton::Crossfade => "crossfade",
            ModeButton::Random => "random",
        };
        // A button whose menu would hold one row isn't a button with a menu.
        // Until something is described, shuffle and the draw lose their hold.
        if button != ModeButton::Crossfade && !crate::settings::similarity_ready() {
            return panel::icon_control(
                icon,
                color,
                panel::Tip::keyed(key, tip),
                move |this: &mut Self, cx| match button {
                    ModeButton::Random => this.play_draw(cx),
                    // Crossfade never gets here: it keeps its menu regardless.
                    _ => this.state.player.update(cx, |p, cx| p.toggle_shuffle(cx)),
                },
                cx,
            );
        }
        // The chevron only hints at the hold; the tooltip says it.
        let tip = panel::Tip::keyed(
            key,
            match button {
                ModeButton::Shuffle => rox_i18n::t!("playback-hold-order", tip = tip).to_string(),
                ModeButton::Crossfade => {
                    rox_i18n::t!("playback-hold-length", tip = tip).to_string()
                }
                ModeButton::Random => rox_i18n::t!("playback-hold-draw", tip = tip).to_string(),
            },
        );
        tip.apply(
            div()
                .relative()
                .p(tokens::ICON_PAD)
                .rounded(tokens::RADIUS)
                .hover(|d| d.bg(palette::bg_control()))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                        this.press_mode(button, event.position, window, cx)
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.release_mode(cx)),
                )
                .child(svg().path(icon).size(px(16.)).text_color(color))
                // Without the corner mark nobody knows to hold.
                .child(
                    div().absolute().top(px(0.)).right(px(0.)).child(
                        svg()
                            .path(icons::CHEVRON_DOWN)
                            .size(px(7.))
                            .text_color(palette::text_faint()),
                    ),
                ),
        )
    }

    /// Remembers where it went down so the menu can hang from there.
    fn press_mode(
        &mut self,
        button: ModeButton,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.press_seq = self.press_seq.wrapping_add(1);
        let seq = self.press_seq;
        self.press = Some(ModePress {
            seq,
            button,
            opened: false,
        });
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SHUFFLE_HOLD).await;
            this.update_in(cx, |this, window, cx| {
                // Only if this exact press is still down: a release or a second press
                // makes this stale.
                let held = this
                    .press
                    .as_ref()
                    .is_some_and(|press| press.seq == seq && !press.opened);
                if !held {
                    return;
                }
                if let Some(press) = this.press.as_mut() {
                    press.opened = true;
                }
                this.open_mode_menu(button, at, window, cx);
            })
            .ok();
        })
        .detach();
    }

    /// A hold already did its work and swallows the click.
    fn release_mode(&mut self, cx: &mut Context<Self>) {
        let Some(press) = self.press.take() else {
            return;
        };
        if press.opened {
            return;
        }
        match press.button {
            ModeButton::Shuffle => self
                .state
                .player
                .update(cx, |player, cx| player.toggle_shuffle(cx)),
            ModeButton::Crossfade => self
                .state
                .player
                .update(cx, |player, cx| player.toggle_crossfade(cx)),
            // The draw isn't a toggle: a press does whichever draw the dropdown last
            // picked.
            ModeButton::Random => self.play_draw(cx),
        }
    }

    fn open_mode_menu(
        &mut self,
        button: ModeButton,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = match button {
            ModeButton::Shuffle => self.shuffle_menu(window, cx),
            ModeButton::Crossfade => self.crossfade_menu(window, cx),
            ModeButton::Random => self.random_menu(window, cx),
        };
        menu.focus_handle(cx).focus(window);
        let subscription = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.mode_menu = None;
            cx.notify();
        });
        self.mode_menu = Some((at, menu, subscription));
        cx.notify();
    }

    /// The same two the Behavior page lists and explains.
    fn shuffle_menu(&self, window: &mut Window, cx: &mut Context<Self>) -> Entity<PopupMenu> {
        let current = self.state.player.read(cx).shuffle_mode();
        let player = self.state.player.clone();
        PopupMenu::build(window, cx, move |menu, _, _| {
            // The check goes on the right: a left check replaces the row's icon
            // (`render_icon`) instead of joining it.
            let mut menu = menu.check_side(Side::Right);
            // The button drops its menu while Similar has nothing to sort by, so
            // there's no disabled row here.
            for mode in ShuffleMode::ALL {
                let player = player.clone();
                menu = menu.item(
                    PopupMenuItem::new(mode.label())
                        .icon(Icon::default().path(mode_icon(mode)))
                        .checked(mode == current)
                        .on_click(move |_, _, cx| {
                            player.update(cx, |player, cx| player.set_shuffle_mode(mode, cx));
                        }),
                );
            }
            menu
        })
    }

    /// The pick belongs to the panel, unlike shuffle's order: nothing else
    /// reads it, and a strip with two draw buttons can have one of each.
    fn random_menu(&self, window: &mut Window, cx: &mut Context<Self>) -> Entity<PopupMenu> {
        let current = self.random_mode();
        let weak = cx.entity().downgrade();
        PopupMenu::build(window, cx, move |menu, _, _| {
            // On the right; see `shuffle_menu`.
            let mut menu = menu.check_side(Side::Right);
            for mode in RandomMode::ALL {
                let weak = weak.clone();
                menu = menu.item(
                    PopupMenuItem::new(mode.label())
                        .icon(Icon::default().path(draw_icon(mode)))
                        .checked(mode == current)
                        .on_click(move |_, _, cx| {
                            let Some(this) = weak.upgrade() else { return };
                            this.update(cx, |this, cx| {
                                this.config.random_mode = mode;
                                cx.notify();
                            });
                        }),
                );
            }
            menu
        })
    }

    /// Lengths rather than a free number: the scrub lives on the Audio page.
    /// The album row is a switch, since a check would read as a sixth length.
    fn crossfade_menu(&self, window: &mut Window, cx: &mut Context<Self>) -> Entity<PopupMenu> {
        let player = self.state.player.read(cx);
        let current = player.crossfade_secs();
        let albums = player.crossfade_albums();
        let entity = self.state.player.clone();
        // Plus the current length when the scrub left it between presets.
        // Rounding 4.3 onto 4 would mark a row that isn't what's playing.
        let mut lengths = CROSSFADE_LENGTHS.to_vec();
        if !lengths.iter().any(|secs| is_length(*secs, current)) {
            lengths.push(current);
            lengths.sort_by(f32::total_cmp);
        }
        PopupMenu::build(window, cx, move |menu, _, _| {
            // On the right; see `shuffle_menu`.
            let mut menu = menu.check_side(Side::Right);
            for secs in lengths.iter().copied() {
                let player = entity.clone();
                menu = menu.item(
                    PopupMenuItem::new(length_label(secs))
                        .icon(Icon::default().path(icons::BLEND))
                        .checked(is_length(secs, current))
                        .on_click(move |_, _, cx| {
                            player.update(cx, |player, cx| player.set_crossfade_secs(secs, cx));
                        }),
                );
            }
            // With the length off there are no boundaries, so the album switch would
            // change nothing.
            if current <= 0.0 {
                return menu;
            }
            let player = entity.clone();
            menu.separator().item(
                PopupMenuItem::element(move |_, _| {
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_between()
                        .gap(tokens::SPACE_MD)
                        .w_full()
                        .child(rox_i18n::t!("playback-crossfade-inside-albums"))
                        // The switch is the row's face: the menu item takes the click anywhere
                        // along the row.
                        .child(panel::toggle_face(albums))
                })
                .on_click(move |_, _, cx| {
                    player.update(cx, |player, cx| player.set_crossfade_albums(!albums, cx));
                }),
            )
        })
    }

    /// The glyph matches the volume strip's, crossed out while muted and one
    /// wave low. Its own control since [`panel::icon_control`] only handles a
    /// left click.
    fn volume_control(&self, volume: f32, muted: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let (speaker, color) = if muted {
            (icons::VOLUME_X, palette::text_faint())
        } else if volume <= 0.5 {
            (icons::VOLUME_1, palette::text())
        } else {
            (icons::VOLUME_2, palette::text())
        };
        // Nothing beside this button shows the level, so the tip carries it and
        // names the right-click.
        let percent = (volume * 100.0).round() as u64;
        let tip = if muted {
            rox_i18n::t!("playback-volume-tip-muted", percent = percent).to_string()
        } else {
            rox_i18n::t!("playback-volume-tip-unmuted", percent = percent).to_string()
        };
        panel::Tip::keyed("volume", tip).apply(
            div()
                .flex_none()
                .p(tokens::ICON_PAD)
                .rounded(tokens::RADIUS)
                .hover(|d| d.bg(palette::bg_control()))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this: &mut Self, _, _, cx| {
                        this.state
                            .player
                            .update(cx, |player, cx| player.toggle_mute(cx));
                    }),
                )
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this: &mut Self, event: &gpui::MouseDownEvent, _, cx| {
                        // The tab panel opens its dropdown on a body right-click; swallow this
                        // or the slider opens under a menu.
                        cx.stop_propagation();
                        this.volume_at = Some(event.position);
                        cx.notify();
                    }),
                )
                .on_scroll_wheel(cx.listener(
                    |this: &mut Self, event: &gpui::ScrollWheelEvent, _, cx| {
                        super::volume_wheel(&this.state.player, event, cx);
                    },
                ))
                .child(svg().path(speaker).size(px(16.)).text_color(color)),
        )
    }

    /// Drawn by hand over the occluding layer rather than built as a menu: a
    /// slider isn't a list of picks.
    fn volume_slider(
        &self,
        at: Point<Pixels>,
        volume: f32,
        muted: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scrub = self.volume_scrub.clone();
        let player = self.state.player.clone();
        let card = div()
            // The card takes its own clicks, so a press on the slider doesn't close
            // it.
            .occlude()
            .flex()
            .items_center()
            .gap(tokens::SPACE_SM)
            .p(tokens::SPACE_SM)
            .rounded(tokens::RADIUS)
            .bg(palette::bg_menu())
            .border_1()
            .border_color(palette::border())
            .shadow_md()
            .child(
                div()
                    // Fixed: this hangs over the strip, so there's no width to fill.
                    .w(VOLUME_POP_W)
                    .flex_none()
                    .h(tokens::CONTROL_H)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this: &mut Self, event: &gpui::MouseDownEvent, _, cx| {
                            this.volume_scrub.begin();
                            if let Some(fraction) = this.volume_scrub.fraction(event.position.x) {
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
                            move |bounds, _, window, _| {
                                panel::paint_slider(volume, muted, bounds, window);
                                panel::scrub_on_paint(&scrub, window, {
                                    let player = player.clone();
                                    move |fraction, cx| {
                                        player.update(cx, |player, cx| {
                                            player.set_volume(fraction, cx)
                                        })
                                    }
                                });
                            },
                        )
                        .size_full(),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::format::format_percent(
                        (volume * 100.0).round() as f64
                    )),
            );
        deferred(
            anchored().child(
                overlay_layer(window)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this: &mut Self, _, _, cx| this.close_volume(cx)),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this: &mut Self, _, _, cx| this.close_volume(cx)),
                    )
                    .child(
                        anchored()
                            .position(at)
                            .snap_to_window_with_margin(px(8.))
                            .child(card),
                    ),
            ),
        )
        .with_priority(1)
        .into_any_element()
    }

    fn close_volume(&mut self, cx: &mut Context<Self>) {
        self.volume_at = None;
        self.volume_scrub.end();
        cx.notify();
    }

    /// Resolves and caches on a track change. No id while nothing plays or
    /// the library doesn't know the file.
    fn current_heart(&mut self, cx: &App) -> (Option<i64>, bool) {
        let Some(key) = TrackSource::Playing.resolve(&self.state, cx) else {
            self.heart = None;
            return (None, false);
        };
        if self.heart.as_ref().map(|heart| &heart.key) != Some(&key) {
            let library = self.state.library.read(cx);
            let id = library.id_for_key(&key);
            let on = id.is_some_and(|id| library.is_favourite(id));
            let rating = id
                .and_then(|id| library.ratings_for(&[id]).get(&id).copied())
                .unwrap_or(0);
            self.heart = Some(Heart {
                key,
                id,
                on,
                rating,
            });
        }
        self.heart
            .as_ref()
            .map_or((None, false), |heart| (heart.id, heart.on))
    }

    fn current_rating(&mut self, cx: &App) -> (Option<i64>, u8) {
        self.current_heart(cx);
        self.heart
            .as_ref()
            .map_or((None, 0), |heart| (heart.id, heart.rating))
    }

    /// The id stays put, so this is one single-track query rather than a
    /// resolve.
    fn refresh_favourite(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.heart.as_ref().and_then(|heart| heart.id) else {
            return;
        };
        let on = self.state.library.read(cx).is_favourite(id);
        if let Some(heart) = self.heart.as_mut() {
            heart.on = on;
        }
        cx.notify();
    }

    /// The id stays put, so this costs one lookup.
    fn refresh_rating(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.heart.as_ref().and_then(|heart| heart.id) else {
            return;
        };
        let rating = self
            .state
            .library
            .read(cx)
            .ratings_for(&[id])
            .get(&id)
            .copied()
            .unwrap_or(0);
        if let Some(heart) = self.heart.as_mut() {
            heart.rating = rating;
        }
        cx.notify();
    }

    /// Similar reads as Random until something has been described, the same
    /// fallback shuffle has. The pick itself stays, so analyzing later brings
    /// it back.
    fn random_mode(&self) -> RandomMode {
        if self.config.random_mode == RandomMode::Similar && !crate::settings::similarity_ready() {
            return RandomMode::Random;
        }
        self.config.random_mode
    }

    /// The player does both draws; this hands over the library.
    fn play_draw(&mut self, cx: &mut Context<Self>) {
        let library = self.state.library.clone();
        let mode = self.random_mode();
        self.state.player.update(cx, |player, cx| match mode {
            RandomMode::Random => player.play_random(&library, cx),
            RandomMode::Similar => player.play_similar(&library, cx),
        });
    }
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
                rox_i18n::t!("playback-buttons"),
                Some(rox_i18n::t!("playback-buttons.description")),
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
                    rox_i18n::t!("playback-play-highlight"),
                    Some(rox_i18n::t!("playback-play-highlight.description")),
                    panel::choices_shared(
                        &[
                            (
                                rox_i18n::t!("playback-highlight-circle"),
                                PlayHighlight::Circle,
                            ),
                            (
                                rox_i18n::t!("playback-highlight-square"),
                                PlayHighlight::Square,
                            ),
                            (rox_i18n::t!("shader-pick-none"), PlayHighlight::None),
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
        let focus = self.focus.clone();
        let body = panel::themed(&chrome, || self.body(window, cx).track_focus(&focus));
        // The afterglow and the A-B pulse outlast what the observer watches, so
        // the panel asks for its own frames.
        if self.outro.is_some() || self.ab_waiting.is_some() {
            window.request_animation_frame();
        }
        body
    }
}

impl TransportPanel {
    fn body(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        let player = self.state.player.read(cx);
        let playing = player.is_playing();
        let active = player.is_active();
        // A station dialling or redialling: the one wait that outlasts a frame,
        // shown on the play button that started it. A file answers None.
        let waiting = player
            .stream_state()
            .filter(|state| matches!(state, StreamState::Opening | StreamState::Reconnecting));
        let volume = player.volume();
        let muted = player.muted();
        // Loop reads through the button: dim off, the accent on, the one-track
        // glyph for single-track loop. The tooltip says it in words too.
        let (loop_icon, loop_color, loop_tip) = match player.loop_mode() {
            LoopMode::Off => (
                icons::REPEAT,
                palette::text_faint(),
                rox_i18n::t!("playback-loop-off"),
            ),
            LoopMode::All => (
                icons::REPEAT,
                palette::accent(),
                rox_i18n::t!("playback-loop-queue"),
            ),
            LoopMode::One => (
                icons::REPEAT_1,
                palette::accent(),
                rox_i18n::t!("playback-loop-track"),
            ),
        };
        // Shuffle's glyph follows the mode, so it says what a press would do;
        // the colour says whether it's on.
        let shuffle_mode = player.shuffle_mode();
        let shuffle_color = if player.shuffle() {
            palette::accent()
        } else {
            palette::text_faint()
        };
        let shuffle_tip = if player.shuffle() {
            rox_i18n::t!(
                "playback-shuffle-on",
                order = shuffle_mode.label().to_lowercase()
            )
            .to_string()
        } else {
            rox_i18n::t!("playback-shuffle-off").to_string()
        };
        let continue_color = if player.continuation_mode() == continuation::Mode::Off {
            palette::text_faint()
        } else {
            palette::accent()
        };
        // The one glyph can't show which strategy is refilling, so the tooltip
        // names it.
        let continue_tip = match player.continuation_mode() {
            continuation::Mode::Off => rox_i18n::t!("playback-continue-off"),
            continuation::Mode::Continue => rox_i18n::t!("playback-continue-down-list"),
            continuation::Mode::Weighted => rox_i18n::t!("playback-continue-weighted"),
        };
        let crossfade_secs = player.crossfade_secs();
        let crossfade_color = if crossfade_secs > 0.0 {
            palette::accent()
        } else {
            palette::text_faint()
        };
        let crossfade_tip = if crossfade_secs > 0.0 {
            rox_i18n::t!(
                "playback-crossfade-tip",
                length = length_label(crossfade_secs)
            )
            .to_string()
        } else {
            rox_i18n::t!("playback-crossfade-off").to_string()
        };
        // The draw has no on/off state, so it's never dim; the glyph and the
        // words follow the pick.
        let random_mode = self.random_mode();
        let random_tip = match random_mode {
            RandomMode::Random => rox_i18n::t!("playback-random-tip-random"),
            RandomMode::Similar => rox_i18n::t!("playback-random-tip-similar"),
        };
        let stop_after_color = if player.stop_after() {
            palette::accent()
        } else {
            palette::text_faint()
        };
        let stop_after_tip = if player.stop_after() {
            rox_i18n::t!("playback-stop-after-armed")
        } else {
            rox_i18n::t!("playback-stop-after-tip")
        };
        // Off, waiting on one mark, or the accent while a section repeats.
        let ab = player.ab_state();
        let ab_color = match ab {
            AbState::Off => palette::text_faint(),
            AbState::ASet(_) => palette::text(),
            AbState::Looping(..) => palette::accent(),
        };
        // The wait for B needs a sign: the first press changed nothing audible.
        // A corner dot breathes until the second mark lands or the cycle drops.
        match ab {
            AbState::ASet(_) => {
                if self.ab_waiting.is_none() {
                    self.ab_waiting = Some(Instant::now());
                }
            }
            _ => self.ab_waiting = None,
        }
        let ab_pulse = self.ab_waiting.map(|since| {
            let phase = since.elapsed().as_secs_f32() * std::f32::consts::TAU / AB_PULSE_SECS;
            AB_PULSE_FLOOR + (1.0 - AB_PULSE_FLOOR) * (0.5 + 0.5 * phase.cos())
        });
        let ab_tip = match ab {
            AbState::Off => rox_i18n::t!("playback-ab-tip-off").to_string(),
            AbState::ASet(a) => rox_i18n::t!("playback-ab-tip-a", a = fmt_time(a)).to_string(),
            AbState::Looping(a, b) => {
                rox_i18n::t!("playback-ab-tip-looping", a = fmt_time(a), b = fmt_time(b))
                    .to_string()
            }
        };
        // A crossfade in flight sweeps across the button that started it, in the
        // direction the queue moved. A boundary fade shows on Next.
        let fade = player.crossfade();
        // Finished means the afterglow; cancelled means gone, since glowing over
        // a stop would celebrate an interruption.
        if fade.is_some() {
            self.outro = None;
            self.last_fade = fade;
        } else if let Some(last) = self.last_fade.take()
            && last.progress() >= OUTRO_FROM
        {
            self.outro = Some((Instant::now(), last.back));
        }
        // Starts at full and falls off with the square, most of the dissolve in
        // the front half.
        let outro = self.outro.and_then(|(at, back)| {
            let t = at.elapsed().as_secs_f32() / tokens::EASE_SECS;
            (t < 1.0).then_some((back, (1.0 - t) * (1.0 - t)))
        });
        if outro.is_none() {
            self.outro = None;
        }
        // Resolving costs a lookup, so only while the heart is shown.
        let (heart_id, heart_on) = if self.config.items.contains(&PlaybackItem::Favourite) {
            self.current_heart(cx)
        } else {
            (None, false)
        };
        let (rating_id, rating_value) = if self.config.items.contains(&PlaybackItem::Rating) {
            self.current_rating(cx)
        } else {
            (None, 0)
        };

        let highlight = self.config.play_highlight;
        let play_ink = if highlight == PlayHighlight::None {
            palette::text()
        } else {
            palette::text_on_accent()
        };
        let mut controls: Vec<AnyElement> = Vec::new();
        for item in self.config.items.clone() {
            controls.push(match item {
                PlaybackItem::Prev => panel::icon_control_fading(
                    icons::SKIP_BACK,
                    palette::text(),
                    panel::Tip::keyed("prev", rox_i18n::t!("playback-item-previous")),
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
                    panel::Tip::keyed("seek-back", rox_i18n::t!("playback-seek-back-tip")),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.seek_by(-10.0)),
                    cx,
                )
                .into_any_element(),
                // The accent fill marks the primary action; the config picks its shape
                // or drops it. The only button with its own key, so its tip trails the
                // shortcut.
                PlaybackItem::Play => panel::Tip::keyed(
                    "play",
                    match waiting {
                        // The same sentences the strip's LIVE mark carries.
                        Some(StreamState::Reconnecting) => {
                            rox_i18n::t!("transport-live-reconnecting")
                        }
                        Some(_) => rox_i18n::t!("transport-live-opening"),
                        None if playing => rox_i18n::t!("playback-pause"),
                        None => rox_i18n::t!("playback-item-play"),
                    },
                )
                .action(&TogglePlayback, PLAYBACK_TIP_SCOPE)
                .apply(
                    div()
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
                        .child(match waiting {
                            // A pause glyph while a station answers says the press did something it
                            // hasn't yet. The spinner is the app's standard wait, at the glyph's
                            // size.
                            Some(_) => Spinner::new().color(play_ink.into()).into_any_element(),

                            None => svg()
                                .path(if playing { icons::PAUSE } else { icons::PLAY })
                                .size_4()
                                .text_color(play_ink)
                                .into_any_element(),
                        }),
                )
                .into_any_element(),
                PlaybackItem::SeekForward => panel::icon_control(
                    icons::FAST_FORWARD,
                    palette::text(),
                    panel::Tip::keyed("seek-forward", rox_i18n::t!("playback-seek-forward-tip")),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.seek_by(10.0)),
                    cx,
                )
                .into_any_element(),
                PlaybackItem::Next => panel::icon_control_fading(
                    icons::SKIP_FORWARD,
                    palette::text(),
                    panel::Tip::keyed("next", rox_i18n::t!("playback-item-next")),
                    fade.filter(|fade| !fade.back),
                    outro
                        .filter(|(back, _)| !*back)
                        .map(|(_, strength)| strength),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, cx| p.next(cx)),
                    cx,
                )
                .into_any_element(),
                // Stop ejects the track and every view over the session goes idle. Dim
                // while nothing is loaded.
                PlaybackItem::Stop => panel::icon_control(
                    icons::STOP,
                    if active {
                        palette::text()
                    } else {
                        palette::text_faint()
                    },
                    panel::Tip::keyed("stop", rox_i18n::t!("playback-stop-tip")),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, cx| p.stop(cx)),
                    cx,
                )
                .into_any_element(),
                PlaybackItem::Volume => self.volume_control(volume, muted, cx).into_any_element(),
                PlaybackItem::Repeat => panel::icon_control(
                    loop_icon,
                    loop_color,
                    // Keyed: the glyph and the words follow the mode, and the id has to
                    // stay fixed under them.
                    panel::Tip::keyed("loop", loop_tip.clone()),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.cycle_loop()),
                    cx,
                )
                .into_any_element(),
                PlaybackItem::Shuffle => self
                    .mode_control(
                        ModeButton::Shuffle,
                        mode_icon(shuffle_mode),
                        shuffle_color,
                        shuffle_tip.clone(),
                        cx,
                    )
                    .into_any_element(),
                // A plain toggle: the strategy is the Behavior page's business. One
                // glyph, since to the ear the strategies differ only in taste.
                PlaybackItem::Continue => panel::icon_control(
                    icons::INFINITY,
                    continue_color,
                    panel::Tip::keyed("continue", continue_tip.clone()),
                    |this: &mut Self, cx| {
                        this.state
                            .player
                            .update(cx, |p, cx| p.toggle_continuation(cx))
                    },
                    cx,
                )
                .into_any_element(),
                // One glyph whatever the length: the colour already says whether
                // anything fades.
                PlaybackItem::Crossfade => self
                    .mode_control(
                        ModeButton::Crossfade,
                        icons::BLEND,
                        crossfade_color,
                        crossfade_tip.clone(),
                        cx,
                    )
                    .into_any_element(),
                PlaybackItem::Random => self
                    .mode_control(
                        ModeButton::Random,
                        draw_icon(random_mode),
                        palette::text(),
                        random_tip.to_string(),
                        cx,
                    )
                    .into_any_element(),
                PlaybackItem::StopAfter => panel::icon_control(
                    icons::SQUARE_DASHED,
                    stop_after_color,
                    panel::Tip::keyed("stop-after", stop_after_tip.clone()),
                    |this: &mut Self, cx| {
                        this.state
                            .player
                            .update(cx, |p, cx| p.toggle_stop_after(cx))
                    },
                    cx,
                )
                .into_any_element(),
                // One button for mark, mark, clear: these controls have no secondary
                // click.
                PlaybackItem::AbRepeat => panel::icon_control(
                    icons::MOVE_HORIZONTAL,
                    ab_color,
                    panel::Tip::keyed("ab-repeat", ab_tip.clone()),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, cx| p.ab_mark(cx)),
                    cx,
                )
                .when_some(ab_pulse, |d, strength| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(2.))
                            .right(px(2.))
                            .size(AB_DOT)
                            .rounded_full()
                            .bg(palette::alpha(
                                palette::accent(),
                                (0xff as f32 * strength) as u8,
                            )),
                    )
                })
                .into_any_element(),
                PlaybackItem::Favourite => {
                    // Dimmed and unclickable reads as broken, so the tip says there's no
                    // track under it.
                    let tip = match (heart_id.is_some(), heart_on) {
                        (false, _) => rox_i18n::t!("transport-favourite-nothing"),
                        (true, true) => rox_i18n::t!("transport-favourite-remove"),
                        (true, false) => rox_i18n::t!("transport-favourite-add"),
                    };
                    panel::Tip::keyed("favourite", tip)
                        .apply(
                            div()
                                .flex_none()
                                .p(tokens::ICON_PAD)
                                .rounded(tokens::RADIUS)
                                .child(
                                    svg()
                                        .path(if heart_on {
                                            icons::HEART_FILLED
                                        } else {
                                            icons::HEART
                                        })
                                        .size(px(16.))
                                        .text_color(if heart_on {
                                            palette::accent()
                                        } else {
                                            palette::text_faint()
                                        }),
                                )
                                // Stays up dimmed so the strip holds its shape while the queue turns
                                // over.
                                .when(heart_id.is_none(), |d| d.opacity(0.4))
                                .when_some(heart_id, |d, id| {
                                    d.cursor_pointer()
                                        .hover(|d| d.bg(palette::bg_control()))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this: &mut Self, _, _, cx| {
                                                this.state.library.update(cx, |library, cx| {
                                                    library.set_favourites(&[id], !heart_on, cx)
                                                });
                                            }),
                                        )
                                }),
                        )
                        .into_any_element()
                }
                PlaybackItem::Rating => {
                    let state = self.state.clone();
                    // Keyed by the shown track so the hover preview matches every other
                    // surface rating it.
                    let key = rating_id.unwrap_or(-1) as u64;
                    let control = rating_ui::control(key, rating_value, move |rating, _, cx| {
                        let Some(id) = rating_id else { return };
                        state
                            .library
                            .update(cx, |library, cx| library.rate(id, rating, cx));
                    });
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .p(tokens::ICON_PAD)
                        // Stays up dimmed so the strip holds its shape.
                        .when(rating_id.is_none(), |d| d.opacity(0.4))
                        .child(control)
                        .into_any_element()
                }
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
            // The occluding layer under it closes the menu on an outside click,
            // `PopoutHost`'s arrangement in panel.rs.
            .when_some(self.mode_menu.as_ref(), |strip, (at, menu, _)| {
                strip.child(
                    deferred(
                        anchored().child(
                            overlay_layer(window).child(
                                anchored()
                                    .position(*at)
                                    .snap_to_window_with_margin(px(8.))
                                    .child(menu.clone()),
                            ),
                        ),
                    )
                    .with_priority(1),
                )
            })
            .when_some(self.volume_at, |strip, at| {
                strip.child(self.volume_slider(at, volume, muted, window, cx))
            })
    }
}

// The playback row is fully composable, so it uses the app's own panel
// floor instead of pinning a width.
transport_panel!(
    TransportPanel,
    "playback",
    rox_i18n::t!("panel-title-playback"),
    min_w = |_: &TransportPanel| rox_dock::resizable::PANEL_MIN_SIZE
);

#[cfg(test)]
mod tests {
    use super::{
        CROSSFADE_LENGTHS, PlaybackItem, RandomMode, TransportConfig, is_length, length_label,
    };

    /// A 4.3 from the scrub isn't the 4: marking the wrong row is worse than
    /// marking none.
    #[test]
    fn a_length_between_the_presets_reads_as_itself() {
        assert_eq!(length_label(0.0), "Off");
        assert_eq!(length_label(4.0), "4 s");
        assert_eq!(length_label(10.0), "10 s");
        assert_eq!(length_label(4.3), "4.3 s");
        // The scrub snaps to tenths, so anything closer is the whole number.
        assert_eq!(length_label(3.999), "4 s");

        assert!(is_length(4.0, 4.0));
        assert!(!is_length(4.0, 4.3));
        assert!(
            !CROSSFADE_LENGTHS
                .iter()
                .any(|preset| is_length(*preset, 4.3))
        );
    }

    #[test]
    fn missing_toggles_default_to_the_stock_strip() {
        let config: TransportConfig = serde_json::from_str("{}").unwrap();
        assert!(config.items == TransportConfig::default().items);
    }

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

    #[test]
    fn continue_and_crossfade_are_opt_in() {
        let stock = TransportConfig::default();
        assert!(!stock.items.contains(&PlaybackItem::Continue));
        assert!(!stock.items.contains(&PlaybackItem::Crossfade));

        let legacy: TransportConfig = serde_json::from_str(r#"{"shuffle": true}"#).unwrap();
        assert!(!legacy.items.contains(&PlaybackItem::Continue));
        assert!(!legacy.items.contains(&PlaybackItem::Crossfade));

        let picked: TransportConfig =
            serde_json::from_str(r#"{"items": ["play", "continue", "crossfade"]}"#).unwrap();
        assert!(
            picked.items
                == vec![
                    PlaybackItem::Play,
                    PlaybackItem::Continue,
                    PlaybackItem::Crossfade
                ]
        );
    }

    #[test]
    fn volume_and_favourite_are_opt_in() {
        let stock = TransportConfig::default();
        assert!(!stock.items.contains(&PlaybackItem::Volume));
        assert!(!stock.items.contains(&PlaybackItem::Favourite));

        let legacy: TransportConfig =
            serde_json::from_str(r#"{"stop": true, "random": true}"#).unwrap();
        assert!(!legacy.items.contains(&PlaybackItem::Volume));
        assert!(!legacy.items.contains(&PlaybackItem::Favourite));

        let picked: TransportConfig =
            serde_json::from_str(r#"{"items": ["volume", "play", "favourite"]}"#).unwrap();
        assert!(
            picked.items
                == vec![
                    PlaybackItem::Volume,
                    PlaybackItem::Play,
                    PlaybackItem::Favourite
                ]
        );

        let saved = serde_json::to_value(&picked).unwrap();
        let back: TransportConfig = serde_json::from_value(saved).unwrap();
        assert!(back.items == picked.items);
    }

    #[test]
    fn the_draw_mode_defaults_to_random_and_round_trips() {
        assert!(TransportConfig::default().random_mode == RandomMode::Random);

        let legacy: TransportConfig = serde_json::from_str(r#"{"random": true}"#).unwrap();
        assert!(legacy.random_mode == RandomMode::Random);

        let picked: TransportConfig =
            serde_json::from_str(r#"{"items": ["random"], "random_mode": "similar"}"#).unwrap();
        assert!(picked.random_mode == RandomMode::Similar);

        let saved = serde_json::to_value(&picked).unwrap();
        let back: TransportConfig = serde_json::from_value(saved).unwrap();
        assert!(back.random_mode == RandomMode::Similar);
    }

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
