//! The playback controls panel: prev, the seek nudges around play/pause,
//! next, and the loop and shuffle modes, plus the optional stop and random
//! buttons.

use std::time::{Duration, Instant};

use gpui::{
    anchored, deferred, div, prelude::*, px, svg, AnyElement, App, Context, DismissEvent, Div,
    Entity, EventEmitter, FocusHandle, Focusable, MouseButton, Pixels, Point, Stateful,
    Subscription, WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::{Icon, Side};
use rox_dock::{Panel, PanelEvent, TabPanel};
use serde::{Deserialize, Serialize};

use rox_playback::engine::LoopMode;

use crate::assets::icons;
use crate::continuation;
use crate::design::{palette, tokens};
use crate::panel::{self, align_row, justify, Align, AppState, PanelChrome, PanelSettings};
use crate::panel_settings;
use crate::player::observe_view;
use crate::settings::ShuffleMode;
use crate::workspace::{TogglePlayback, PLAYBACK_TIP_SCOPE};

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
    /// The continuation button: whether a queue that runs out keeps playing,
    /// and which strategy refills it (ADR 17).
    Continue,
    /// The crossfade button: whether track boundaries overlap, and for how
    /// long (ADR 19).
    Crossfade,
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
        label: "Continue",
        icon: Some(icons::INFINITY),
        value: PlaybackItem::Continue,
    },
    panel::ArrangeSpec {
        label: "Crossfade",
        icon: Some(icons::BLEND),
        value: PlaybackItem::Crossfade,
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
        // The stock strip in the order it always rendered: nudges around
        // play, the modes trailing. Stop, random, continue, crossfade and
        // stop-after are opt-in from the panel's menu.
        //
        // Continue is opt-in even though continuation ships on (ADR 17). Its
        // strategy is picked on the Behavior page, where each one explains
        // itself, and a default strip carrying a button for every mode that
        // quietly does something is how a transport turns into a dashboard.
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
                // Continue and crossfade aren't here: neither ships in the
                // stock strip, so a layout from before those buttons existed
                // comes back looking exactly like a fresh install rather than
                // growing controls nobody asked for.
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
    /// Bumped on every press of a mode button, so a stale hold check can
    /// tell it belongs to a press that is already over.
    press_seq: u64,
    /// The mode press in flight, if any. Taken on release, so a press that
    /// turned into a hold doesn't also toggle.
    press: Option<ModePress>,
    /// The mode menu while it's open, hung from the point the press
    /// started. The `PopoutHost` dock menu's shape (`panel.rs`), because a
    /// gpui-component context menu opens on right-click and can't be asked
    /// to open by anything else.
    mode_menu: Option<(Point<Pixels>, Entity<PopupMenu>, Subscription)>,
    _player_changed: Subscription,
}

/// A button whose click toggles something and whose hold opens the shades of
/// it. Two of them: shuffle picks the order it puts the queue in, crossfade
/// picks how long one track lies over the next.
///
/// Continue isn't one. Its strategies differ in kind rather than degree and
/// they need a sentence each, which is what the Behavior page is for; these
/// two are a short list of shades of the same thing, which is what a hold
/// menu is good at.
#[derive(Clone, Copy, PartialEq)]
enum ModeButton {
    Shuffle,
    Crossfade,
}

/// A press on a mode button that hasn't been released yet.
struct ModePress {
    /// Which press this is. The delayed hold check compares against the
    /// panel's counter, so a press that was released and replaced before the
    /// delay elapsed can't open a menu for the press after it.
    seq: u64,
    /// Which button is down, so the hold opens the right list.
    button: ModeButton,
    /// Whether the hold already fired. Set by the delayed check, read by the
    /// release so it knows not to toggle.
    opened: bool,
}

/// The glyph a shuffle mode wears. Random keeps the crossed arrows shuffle
/// has always meant. Similar takes the radio, which is both the metaphor
/// people already have for "more of this" and where the mode is going.
fn mode_icon(mode: ShuffleMode) -> &'static str {
    match mode {
        ShuffleMode::Random => icons::SHUFFLE,
        ShuffleMode::Similar => icons::RADIO,
    }
}

/// The crossfade lengths the hold menu offers, in seconds, zero being off.
/// A short list of round numbers: the Audio page's scrub is where a length
/// between these gets set, and this button is for reaching the common ones
/// without leaving the music.
const CROSSFADE_LENGTHS: [f32; 5] = [0.0, 2.0, 4.0, 6.0, 10.0];

/// Whether two crossfade lengths are the same one. Floats, and the scrub
/// writes tenths, so this is the resolution the readout shows rather than
/// bit equality.
fn is_length(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.05
}

/// A crossfade length as the menu says it: off, whole seconds where the
/// number is one, and a tenth where the scrub left it between.
fn length_label(secs: f32) -> String {
    if secs <= 0.0 {
        "Off".to_string()
    } else if is_length(secs, secs.round()) {
        format!("{secs:.0} s")
    } else {
        format!("{secs:.1} s")
    }
}

/// How long a mode button has to be held before its menu opens. Long
/// enough that a normal click never reaches it, short enough that the hold
/// doesn't feel broken.
const SHUFFLE_HOLD: Duration = Duration::from_millis(350);

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
            press: None,
            mode_menu: None,
            press_seq: 0,
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
            ("Continue Button", PlaybackItem::Continue),
            ("Crossfade Button", PlaybackItem::Crossfade),
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

    /// A mode button: a plain click toggles it, and holding it opens the
    /// shades behind it.
    ///
    /// Its own control rather than [`panel::icon_control`] for two reasons.
    /// That one fires on mouse down, and a hold has to be able to swallow the
    /// click it started; and the corner arrow needs a positioned child, which
    /// the shared button has no room for.
    fn mode_control(
        &self,
        button: ModeButton,
        icon: &'static str,
        color: gpui::Rgba,
        tip: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        // A button whose menu would hold one row isn't a button with a menu.
        // Shuffle loses its hold while nothing has been described, since
        // Random is then the only order there is, and crossfade never has
        // one to lose.
        if button == ModeButton::Shuffle && !crate::settings::similarity_ready() {
            return panel::icon_control(
                icon,
                color,
                panel::Tip::keyed("shuffle", tip),
                |this: &mut Self, cx| this.state.player.update(cx, |p, cx| p.toggle_shuffle(cx)),
                cx,
            );
        }
        // The hold is the only way to the modes behind the button, and the
        // corner chevron can only hint that there's something there. The
        // tooltip is where it gets said.
        let tip = panel::Tip::keyed(
            match button {
                ModeButton::Shuffle => "shuffle",
                ModeButton::Crossfade => "crossfade",
            },
            match button {
                ModeButton::Shuffle => format!("{tip}. Hold to pick an order"),
                ModeButton::Crossfade => format!("{tip}. Hold to pick a length"),
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
                // The corner mark: without it nothing says the button has
                // modes behind it, and a hold nobody knows about is a hold
                // nobody does.
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

    /// Start a press: arm the hold, and remember where it went down so the
    /// menu can hang from there.
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
                // Only if this exact press is still down: a release, or a
                // second press, both make this answer stale.
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

    /// Finish a press. A hold already did its work and swallows the click;
    /// anything shorter is the plain toggle.
    fn release_mode(&mut self, cx: &mut Context<Self>) {
        let Some(press) = self.press.take() else {
            return;
        };
        if press.opened {
            return;
        }
        self.state
            .player
            .update(cx, |player, cx| match press.button {
                ModeButton::Shuffle => player.toggle_shuffle(cx),
                ModeButton::Crossfade => player.toggle_crossfade(cx),
            });
    }

    /// The mode menu, hung from where the press started.
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
        };
        menu.focus_handle(cx).focus(window);
        let subscription = cx.subscribe(&menu, |this, _, _: &DismissEvent, cx| {
            this.mode_menu = None;
            cx.notify();
        });
        self.mode_menu = Some((at, menu, subscription));
        cx.notify();
    }

    /// The orders shuffle can put the upcoming queue in. The same two the
    /// Behavior page lists, which is where they're explained; this is the
    /// swap without the trip.
    fn shuffle_menu(&self, window: &mut Window, cx: &mut Context<Self>) -> Entity<PopupMenu> {
        let current = self.state.player.read(cx).shuffle_mode();
        let player = self.state.player.clone();
        PopupMenu::build(window, cx, move |menu, _, _| {
            // The check goes on the right because these rows carry their own
            // glyphs. A left check replaces the icon rather than joining it
            // (`render_icon`), so a row with an icon silently loses its mark,
            // which is exactly what was happening here: the menu said what
            // the modes were and never which one was on.
            let mut menu = menu.check_side(Side::Right);
            // Every order this offers can run: the button drops its menu
            // entirely while Similar has nothing to sort by, so there's no
            // disabled row to explain here.
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

    /// How long one track lies over the next (ADR 19), and whether an
    /// album's own boundaries get the fade too.
    ///
    /// Lengths rather than a free number, because a scrub belongs on the
    /// Audio page, which owns the same two knobs and writes through the same
    /// player. The album row wears a switch rather than a checkmark: it isn't
    /// one of the lengths, it's the other knob, and a check in a list of
    /// picks would read as a sixth length.
    fn crossfade_menu(&self, window: &mut Window, cx: &mut Context<Self>) -> Entity<PopupMenu> {
        let player = self.state.player.read(cx);
        let current = player.crossfade_secs();
        let albums = player.crossfade_albums();
        let entity = self.state.player.clone();
        // The presets, plus the length itself when the Audio page's scrub
        // left it between them. A menu that can't mark what's set reads as
        // though nothing is, and rounding 4.3 onto the 4 would be worse: it
        // would mark a row that isn't what's playing.
        let mut lengths = CROSSFADE_LENGTHS.to_vec();
        if !lengths.iter().any(|secs| is_length(*secs, current)) {
            lengths.push(current);
            lengths.sort_by(f32::total_cmp);
        }
        PopupMenu::build(window, cx, move |menu, _, _| {
            // The check on the right, so it joins the glyph instead of
            // replacing it; see `shuffle_menu`.
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
            // The album switch only while something is fading. With the
            // length off there are no boundaries for it to take, so the row
            // would be a switch that changes nothing.
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
                        .child("Inside Albums")
                        // The switch is the row's face, not a control of its
                        // own: the menu item takes the click, so a press
                        // anywhere along the row flips it.
                        .child(panel::toggle_face(albums))
                })
                .on_click(move |_, _, cx| {
                    player.update(cx, |player, cx| player.set_crossfade_albums(!albums, cx));
                }),
            )
        })
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
        // accent while on, the one-track glyph for single-track loop. The
        // tooltip carries the same state in words, since a dim glyph and an
        // accent one only differ once you've seen both.
        let (loop_icon, loop_color, loop_tip) = match player.loop_mode() {
            LoopMode::Off => (icons::REPEAT, palette::text_faint(), "Loop off"),
            LoopMode::All => (icons::REPEAT, palette::accent(), "Loop the queue"),
            LoopMode::One => (icons::REPEAT_1, palette::accent(), "Loop this track"),
        };
        // Shuffle reads the same way: dim while off, the accent while on.
        // Its glyph follows the mode rather than the on/off state, so the
        // button says what it would do before you press it; the colour is
        // what says whether it's doing it.
        let shuffle_mode = player.shuffle_mode();
        let shuffle_color = if player.shuffle() {
            palette::accent()
        } else {
            palette::text_faint()
        };
        let shuffle_tip = if player.shuffle() {
            format!("Shuffle on, {} order", shuffle_mode.label().to_lowercase())
        } else {
            "Shuffle off".to_string()
        };
        // Continuation the same: dim while off, the accent while something
        // is standing by to refill the queue.
        let continue_color = if player.continuation_mode() == continuation::Mode::Off {
            palette::text_faint()
        } else {
            palette::accent()
        };
        // Which strategy is refilling matters here in a way the one glyph
        // can't show, so the tooltip names it.
        let continue_tip = match player.continuation_mode() {
            continuation::Mode::Off => "Keep playing off",
            continuation::Mode::Continue => "Keep playing, on down the list",
            continuation::Mode::Weighted => "Keep playing, never played first",
        };
        // Crossfade reads the same way: dim at zero length, the accent once
        // boundaries are overlapping.
        let crossfade_secs = player.crossfade_secs();
        let crossfade_color = if crossfade_secs > 0.0 {
            palette::accent()
        } else {
            palette::text_faint()
        };
        let crossfade_tip = if crossfade_secs > 0.0 {
            format!("Crossfade {}", length_label(crossfade_secs))
        } else {
            "Crossfade off".to_string()
        };
        // Stop-after too: dim until armed, the accent while it waits.
        let stop_after_color = if player.stop_after() {
            palette::accent()
        } else {
            palette::text_faint()
        };
        let stop_after_tip = if player.stop_after() {
            "Stop after this track, armed"
        } else {
            "Stop after this track"
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
                    "Previous",
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
                    "Back 10 seconds",
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.seek_by(-10.0)),
                    cx,
                )
                .into_any_element(),
                // Play/pause is the primary action, so it gets the accent
                // fill while everything around it stays flat; the config
                // picks the fill's shape, or drops it to match the
                // neighbors.
                // The one button here that has a key of its own, so its tip
                // trails the shortcut.
                PlaybackItem::Play => {
                    panel::Tip::keyed("play", if playing { "Pause" } else { "Play" })
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
                                .child(
                                    svg()
                                        .path(if playing { icons::PAUSE } else { icons::PLAY })
                                        .size_4()
                                        .text_color(if highlight == PlayHighlight::None {
                                            palette::text()
                                        } else {
                                            palette::text_on_accent()
                                        }),
                                ),
                        )
                        .into_any_element()
                }
                PlaybackItem::SeekForward => panel::icon_control(
                    icons::FAST_FORWARD,
                    palette::text(),
                    "Forward 10 seconds",
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.seek_by(10.0)),
                    cx,
                )
                .into_any_element(),
                PlaybackItem::Next => panel::icon_control_fading(
                    icons::SKIP_FORWARD,
                    palette::text(),
                    "Next",
                    fade.filter(|fade| !fade.back),
                    outro
                        .filter(|(back, _)| !*back)
                        .map(|(_, strength)| strength),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, cx| p.next(cx)),
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
                    "Stop and unload the track",
                    |this: &mut Self, cx| this.state.player.update(cx, |p, cx| p.stop(cx)),
                    cx,
                )
                .into_any_element(),
                PlaybackItem::Repeat => panel::icon_control(
                    loop_icon,
                    loop_color,
                    // Keyed, since the glyph and the words both follow the
                    // mode and the id has to sit still under them.
                    panel::Tip::keyed("loop", loop_tip),
                    |this: &mut Self, cx| this.state.player.update(cx, |p, _| p.cycle_loop()),
                    cx,
                )
                .into_any_element(),
                // Shuffle's glyph follows its order and a hold swaps it; the
                // colour is what says whether it's on.
                PlaybackItem::Shuffle => self
                    .mode_control(
                        ModeButton::Shuffle,
                        mode_icon(shuffle_mode),
                        shuffle_color,
                        shuffle_tip.clone(),
                        cx,
                    )
                    .into_any_element(),
                // Continue is a plain toggle: which strategy refills the
                // queue is the Behavior page's business, where each one has
                // room to say what it does. One glyph whatever the strategy,
                // unlike shuffle above, because Continue and Weighted mean
                // the same thing to the ear (the music doesn't stop) and
                // differ only in taste.
                PlaybackItem::Continue => panel::icon_control(
                    icons::INFINITY,
                    continue_color,
                    panel::Tip::keyed("continue", continue_tip),
                    |this: &mut Self, cx| {
                        this.state
                            .player
                            .update(cx, |p, cx| p.toggle_continuation(cx))
                    },
                    cx,
                )
                .into_any_element(),
                // Crossfade holds like shuffle, and keeps one glyph whatever
                // the length: the lengths differ by degree, and the colour
                // already says whether anything is fading at all.
                PlaybackItem::Crossfade => self
                    .mode_control(
                        ModeButton::Crossfade,
                        icons::BLEND,
                        crossfade_color,
                        crossfade_tip.clone(),
                        cx,
                    )
                    .into_any_element(),
                PlaybackItem::Random => panel::icon_control(
                    icons::DICE,
                    palette::text(),
                    "Play a random track",
                    |this: &mut Self, cx| this.play_random(cx),
                    cx,
                )
                .into_any_element(),
                PlaybackItem::StopAfter => panel::icon_control(
                    icons::SQUARE_DASHED,
                    stop_after_color,
                    panel::Tip::keyed("stop-after", stop_after_tip),
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
            // The shuffle menu, over everything and pinned where the hold
            // started. The occluding layer under it is what closes the menu
            // on an outside click, `PopoutHost`'s arrangement in panel.rs.
            .when_some(self.mode_menu.as_ref(), |strip, (at, menu, _)| {
                strip.child(
                    deferred(
                        anchored().child(
                            div().size_full().occlude().child(
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
    use super::{is_length, length_label, PlaybackItem, TransportConfig, CROSSFADE_LENGTHS};

    /// The hold menu has to be able to mark a length the Audio page's scrub
    /// wrote, which is the case the presets alone can't cover: a 4.3 is not
    /// the 4, and saying so is the difference between a menu that marks
    /// nothing and one that marks the wrong row.
    #[test]
    fn a_length_between_the_presets_reads_as_itself() {
        assert_eq!(length_label(0.0), "Off");
        assert_eq!(length_label(4.0), "4 s");
        assert_eq!(length_label(10.0), "10 s");
        assert_eq!(length_label(4.3), "4.3 s");
        // The scrub lands on tenths, so anything closer than that to a whole
        // number is that number rather than a trailing zero.
        assert_eq!(length_label(3.999), "4 s");

        assert!(is_length(4.0, 4.0));
        assert!(!is_length(4.0, 4.3));
        assert!(!CROSSFADE_LENGTHS
            .iter()
            .any(|preset| is_length(*preset, 4.3)));
    }

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

    /// Continue and crossfade are opt-in, so neither the stock strip nor a
    /// layout from before they existed carries one; a layout that names one
    /// keeps it.
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
