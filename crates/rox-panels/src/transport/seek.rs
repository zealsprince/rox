//! The seek strip panel: a track line with the played side in the accent
//! and a playhead, click or drag to seek, the elapsed and remaining clocks
//! at its ends.
//!
//! A station draws on the same line once the engine holds a timeshift tape
//! for it. The strip spans the whole buffer the setting allows, the live
//! edge is its right end, and the untaped part is an unreachable dashed
//! lead-in. Anything keyed to a track position (bookmarks, the scrobble
//! threshold, A-B) stays off there.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Instant;

use gpui::{
    AnyElement, App, Background, Bounds, Context, Corners, Div, Entity, EventEmitter, FocusHandle,
    Focusable, FontFeatures, MouseButton, MouseMoveEvent, Pixels, Stateful, Subscription,
    WeakEntity, Window, canvas, div, fill, linear_color_stop, linear_gradient, point, prelude::*,
    px, relative, size,
};
use gpui_component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_component::tooltip::Tooltip;
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::bookmarks::Bookmark;
use rox_library::cue::TrackKey;
use rox_panel_api::{cue_ui, position_bound};
use rox_playback::{LiveGap, LiveMark, Shift, StreamState};
use rox_services::cues::{Cue, CuesChanged};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::bookmark_ui;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::panel::{self, AppState, PanelChrome, PanelSettings, ScrubState, ValueEdit};
use crate::panel_settings;
use crate::player::{Player, fmt_time, fmt_time_padded, song_clock};
use crate::settings::ui as settings_ui;

use super::{default_true, transport_panel};

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeekItem {
    Elapsed,
    Strip,
    Ending,
    /// Pairs with the elapsed clock for "elapsed, total" without giving up
    /// the countdown.
    Duration,
    /// A flexible gap; a row holds as many as the layout needs.
    Spacer,
    /// A spacer with a hairline across its gap.
    Divider,
    /// Everything after it drops to a second row: the stacked layouts.
    Break,
}

/// What the quick Show Timings toggle moves as a pair.
const CLOCKS: [SeekItem; 2] = [SeekItem::Elapsed, SeekItem::Ending];

/// Stock order: where a menu toggle slots a re-shown piece back in.
const ITEMS: &[panel::ArrangeSpec<SeekItem>] = &[
    panel::ArrangeSpec {
        key: "seek-item-elapsed",
        icon: Some(icons::CLOCK),
        value: SeekItem::Elapsed,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "seek-item-strip",
        icon: Some(icons::AUDIO_LINES),
        value: SeekItem::Strip,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "seek-item-ending",
        icon: Some(icons::CLOCK),
        value: SeekItem::Ending,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "seek-item-duration",
        icon: Some(icons::CLOCK),
        value: SeekItem::Duration,
        repeats: false,
    },
    panel::ArrangeSpec {
        key: "head-piece-spacer",
        icon: Some(icons::MOVE_HORIZONTAL),
        value: SeekItem::Spacer,
        repeats: true,
    },
    panel::ArrangeSpec {
        key: "head-piece-divider",
        icon: Some(icons::MINUS),
        value: SeekItem::Divider,
        repeats: true,
    },
];

/// The untaped part of a station's buffer, left of the tape.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LeadIn {
    /// The default: it says the bar is filling, not growing.
    #[default]
    Dashed,
    /// For a thin strip where dashes read as noise.
    Faint,
    /// The bar is only ever as long as the tape: a strip that grows.
    Hidden,
}

/// Reads through [`SeekConfigDump`] so layouts from before the ordered
/// list still load.
#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "SeekConfigDump")]
pub struct SeekConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// The Ending clock shows the full duration instead of the time left.
    pub show_total: bool,
    /// The Last.fm scrobble threshold. Only draws while scrobbling is on.
    pub scrobble_marker: bool,
    /// Chevrons under the line, each a seek and a right-click menu.
    pub bookmarks: bool,
    pub thickness: f32,
    pub rounding: f32,
    pub playhead_width: f32,
    /// The playhead spans the strip's full height; off, it hugs the line.
    pub playhead_full: bool,
    /// Centered on the line; 0 lets it fill the panel.
    pub playhead_max: f32,
    pub live_lead_in: LeadIn,
    /// The gap after each dash matches it.
    pub live_dash_len: f32,
    /// Off by stock: a strip that moves on its own should be asked for.
    pub live_sweep: bool,
    /// Display order; one not listed is hidden.
    pub items: Vec<SeekItem>,
}

impl Default for SeekConfig {
    fn default() -> Self {
        SeekConfig {
            chrome: PanelChrome::default(),
            show_total: false,
            scrobble_marker: false,
            bookmarks: true,
            thickness: tokens::SEEK_STRIP_H,
            rounding: 0.0,
            playhead_width: tokens::PLAYHEAD_W,
            playhead_full: true,
            playhead_max: 0.0,
            live_lead_in: LeadIn::default(),
            live_dash_len: DASH_LEN_DEFAULT,
            live_sweep: false,
            items: vec![SeekItem::Elapsed, SeekItem::Strip, SeekItem::Ending],
        }
    }
}

/// Newer layouts write the ordered list. Older ones had a `timings`
/// toggle for both clocks.
#[derive(Deserialize)]
struct SeekConfigDump {
    #[serde(flatten)]
    chrome: PanelChrome,
    #[serde(default)]
    show_total: bool,
    #[serde(default)]
    scrobble_marker: bool,
    #[serde(default = "default_true")]
    bookmarks: bool,
    #[serde(default = "default_thickness")]
    thickness: f32,
    #[serde(default)]
    rounding: f32,
    #[serde(default = "default_playhead_width")]
    playhead_width: f32,
    #[serde(default = "default_true")]
    playhead_full: bool,
    #[serde(default)]
    playhead_max: f32,
    #[serde(default)]
    live_lead_in: LeadIn,
    #[serde(default = "default_dash_len")]
    live_dash_len: f32,
    #[serde(default)]
    live_sweep: bool,
    #[serde(default)]
    items: Option<Vec<SeekItem>>,
    #[serde(default = "default_true")]
    timings: bool,
}

fn default_thickness() -> f32 {
    tokens::SEEK_STRIP_H
}

fn default_playhead_width() -> f32 {
    tokens::PLAYHEAD_W
}

/// Four is a dash at the usual thickness. Past the top it reads as a
/// measure, and at one pixel it turns into a comb.
const DASH_LEN_DEFAULT: f32 = 4.0;
const DASH_LEN_MIN: f32 = 1.0;
const DASH_LEN_MAX: f32 = 24.0;

fn default_dash_len() -> f32 {
    DASH_LEN_DEFAULT
}

impl From<SeekConfigDump> for SeekConfig {
    fn from(dump: SeekConfigDump) -> Self {
        let items = match dump.items {
            // Deduped per row: the catalog has no break (it's the editor's row
            // boundary), and each row may hold its own copy of a piece.
            Some(items) => items
                .split(|i| matches!(i, SeekItem::Break))
                .map(|row| panel::dedup(ITEMS, row.to_vec()))
                .collect::<Vec<_>>()
                .join(&SeekItem::Break),
            None if dump.timings => {
                vec![SeekItem::Elapsed, SeekItem::Strip, SeekItem::Ending]
            }
            None => vec![SeekItem::Strip],
        };
        SeekConfig {
            chrome: dump.chrome,
            show_total: dump.show_total,
            scrobble_marker: dump.scrobble_marker,
            bookmarks: dump.bookmarks,
            thickness: dump.thickness,
            rounding: dump.rounding,
            playhead_width: dump.playhead_width,
            playhead_full: dump.playhead_full,
            playhead_max: dump.playhead_max,
            live_lead_in: dump.live_lead_in,
            live_dash_len: dump.live_dash_len,
            live_sweep: dump.live_sweep,
            items,
        }
    }
}

pub struct SeekStripPanel {
    state: AppState,
    config: SeekConfig,
    scrub: ScrubState,
    thickness_scrub: ScrubState,
    rounding_scrub: ScrubState,
    playhead_scrub: ScrubState,
    playhead_max_scrub: ScrubState,
    value_edit: ValueEdit,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// The row before Show Timings hid the clocks, so turning them back on
    /// restores their place. Panel state, not config.
    timings_stash: Option<Vec<SeekItem>>,
    /// Re-read on a track change or a bookmark edit, not every pump tick.
    marks: Vec<Bookmark>,
    marks_key: Option<TrackKey>,
    hover_mark: Option<i64>,
    /// Cached like the bookmarks.
    cues: Vec<Cue>,
    cues_key: Option<TrackKey>,
    hovered_cue: Option<u64>,
    /// Kept apart from the cue hover: both outlive a render, and sharing
    /// would leave a stale id on a switch from a file to a stream.
    hovered_song: Option<u64>,
    /// Only the song chevrons write it: they're the only marks that move.
    pointer_at: Option<f32>,
    dash_scrub: ScrubState,
    /// The insert menu builds a frame after the press and never sees the
    /// event, so the position is parked here.
    insert_at_ms: Arc<AtomicU32>,
    /// Time zero for the LIVE pulse. The strip redraws off the pump, and a
    /// stream that hasn't opened isn't moving it.
    epoch: Instant,
    _player_changed: Subscription,
    _library_changed: Subscription,
    _cues_changed: Subscription,
}

impl SeekStripPanel {
    pub fn new(state: AppState, config: SeekConfig, cx: &mut Context<Self>) -> Self {
        // The clock and playhead move every tick, so this uses the raw per-pump
        // notify, not the gated observe.
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        // A bookmark edit anywhere drops the cached marks.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(
                    event,
                    LibraryEvent::BookmarksChanged | LibraryEvent::Updated
                ) {
                    this.marks_key = None;
                    cx.notify();
                }
            },
        );
        // The event names its track, so a strip on another song keeps its set.
        let _cues_changed = cx.subscribe(
            &state.cues,
            |this: &mut Self, _, event: &CuesChanged, cx| {
                if this.cues_key.as_ref() == Some(&event.key) {
                    this.cues_key = None;
                    cx.notify();
                }
            },
        );
        SeekStripPanel {
            state,
            config,
            scrub: ScrubState::default(),
            thickness_scrub: ScrubState::default(),
            rounding_scrub: ScrubState::default(),
            playhead_scrub: ScrubState::default(),
            playhead_max_scrub: ScrubState::default(),
            value_edit: ValueEdit::default(),
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            timings_stash: None,
            marks: Vec::new(),
            marks_key: None,
            hover_mark: None,
            cues: Vec::new(),
            cues_key: None,
            hovered_cue: None,
            hovered_song: None,
            pointer_at: None,
            dash_scrub: ScrubState::default(),
            insert_at_ms: Arc::new(AtomicU32::new(0)),
            epoch: Instant::now(),
            _player_changed,
            _library_changed,
            _cues_changed,
        }
    }

    /// Once per track and after an edit, so the per-tick repaint never
    /// touches the database.
    fn marks_for(&mut self, key: &TrackKey, cx: &App) -> &[Bookmark] {
        if self.marks_key.as_ref() != Some(key) {
            self.marks = self.state.library.read(cx).bookmarks_for(key);
            self.marks_key = Some(key.clone());
            self.hover_mark = None;
        }
        &self.marks
    }

    fn cues_for(&mut self, key: &TrackKey, cx: &App) -> &[Cue] {
        if self.cues_key.as_ref() != Some(key) {
            self.cues = self.state.cues.read(cx).for_key(key);
            self.cues_key = Some(key.clone());
            self.hovered_cue = None;
        }

        &self.cues
    }

    /// On a station the fraction is a point in the tape, measured back from
    /// the live edge. Read fresh off the player: a drag outlives the paint
    /// that started it.
    fn seek_at(&self, fraction: f32, cx: &App) {
        let player = self.state.player.read(cx);
        match player.now_playing().and_then(|now| now.shift) {
            Some(shift) => player.seek_live(shift_behind(fraction, &shift)),

            None => panel::seek_fraction(&self.state.player, fraction, cx),
        }
    }

    /// A station's marks walk left under a still pointer and fire no event,
    /// so the hover is checked against the pointer on every paint instead of
    /// waiting for a hover-out.
    fn settle_song_hover(&mut self, marks: &[cue_ui::CueMark]) {
        let Some(id) = self.hovered_song else {
            return;
        };

        // In fractions, like the pointer and the mark; only the slot's half
        // width crosses over.
        let still_on = match (self.pointer_at, self.scrub.width()) {
            (Some(at), Some(width)) if width > 0.0 => marks
                .iter()
                .find(|mark| mark.id == id)
                .is_some_and(|mark| ((at - mark.fraction) * width).abs() <= SONG_HIT_W / 2.0),

            _ => false,
        };

        if !still_on {
            self.hovered_song = None;
        }
    }

    fn timings_shown(&self) -> bool {
        self.config
            .items
            .iter()
            .any(|i| matches!(i, SeekItem::Elapsed | SeekItem::Ending))
    }

    /// The row they were in survives the round trip.
    fn toggle_timings(&mut self) {
        self.config.items =
            panel::toggled_stashed(ITEMS, &self.config.items, &mut self.timings_stash, &CLOCKS);
    }

    /// Timings means both clocks at once; the arrange editor splits them.
    fn config_menu(
        &self,
        menu: PopupMenu,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let weak = cx.entity().downgrade();
        let timings = self.timings_shown();
        let menu = menu.item(
            PopupMenuItem::new(rox_i18n::t!("seek-show-timings"))
                .checked(timings)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.toggle_timings();
                        cx.notify();
                    });
                }),
        );
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new(rox_i18n::t!("seek-scrobble-marker"))
                .checked(self.config.scrobble_marker)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.scrobble_marker = !this.config.scrobble_marker;
                        cx.notify();
                    });
                }),
        );
        let weak = cx.entity().downgrade();
        menu.item(
            PopupMenuItem::new(rox_i18n::t!("seek-bookmarks"))
                .checked(self.config.bookmarks)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.bookmarks = !this.config.bookmarks;
                        cx.notify();
                    });
                }),
        )
    }
}

impl PanelSettings for SeekStripPanel {
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
        let layout = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_block(
                rox_i18n::t!("transport-pieces"),
                Some(rox_i18n::t!("transport-pieces.description")),
                None,
                panel::arrange_rows_editor(
                    "seek-items",
                    ITEMS,
                    &editor_rows(&self.config.items),
                    None,
                    |this: &mut Self, rows, cx| {
                        this.config.items = rows.join(&SeekItem::Break);
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("seek-thickness"),
                Some(rox_i18n::t!("seek-thickness.description")),
                settings_ui::scalar(
                    &self.thickness_scrub,
                    &self.value_edit,
                    self.config.thickness,
                    settings_ui::span(1., 16., " px"),
                    |this: &mut Self, thickness, cx| {
                        this.config.thickness = thickness;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("seek-rounding"),
                Some(rox_i18n::t!("seek-rounding.description")),
                settings_ui::scalar(
                    &self.rounding_scrub,
                    &self.value_edit,
                    self.config.rounding,
                    settings_ui::span(0., 8., " px"),
                    |this: &mut Self, rounding, cx| {
                        this.config.rounding = rounding;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("seek-playhead"),
                Some(rox_i18n::t!("seek-playhead.description")),
                panel::choices_shared(
                    &[
                        (rox_i18n::t!("seek-playhead-full"), true),
                        (rox_i18n::t!("seek-playhead-line"), false),
                    ],
                    self.config.playhead_full,
                    |this: &mut Self, full, cx| {
                        this.config.playhead_full = full;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("seek-playhead-width"),
                Some(rox_i18n::t!("seek-playhead-width.description")),
                settings_ui::scalar(
                    &self.playhead_scrub,
                    &self.value_edit,
                    self.config.playhead_width,
                    settings_ui::span(1., 8., " px"),
                    |this: &mut Self, width, cx| {
                        this.config.playhead_width = width;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(self.config.playhead_full, |d| {
                d.child(panel::setting_row(
                    rox_i18n::t!("seek-playhead-max-height"),
                    Some(rox_i18n::t!("seek-playhead-max-height.description")),
                    settings_ui::scalar(
                        &self.playhead_max_scrub,
                        &self.value_edit,
                        self.config.playhead_max,
                        settings_ui::span(0., 100., " px"),
                        |this: &mut Self, max, cx| {
                            this.config.playhead_max = max;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .when(self.config.items.contains(&SeekItem::Ending), |d| {
                d.child(panel::setting_row(
                    rox_i18n::t!("seek-ending"),
                    Some(rox_i18n::t!("seek-ending.description")),
                    panel::choices_shared(
                        &[
                            (rox_i18n::t!("seek-ending-remaining"), false),
                            (rox_i18n::t!("seek-ending-total"), true),
                        ],
                        self.config.show_total,
                        |this: &mut Self, show_total, cx| {
                            this.config.show_total = show_total;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .child(panel::setting_row(
                rox_i18n::t!("seek-scrobble-marker"),
                Some(rox_i18n::t!("seek-scrobble-marker.description")),
                panel::toggle(
                    self.config.scrobble_marker,
                    |this: &mut Self, on, cx| {
                        this.config.scrobble_marker = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("seek-bookmarks"),
                Some(rox_i18n::t!("seek-bookmarks.description")),
                panel::toggle(
                    self.config.bookmarks,
                    |this: &mut Self, on, cx| {
                        this.config.bookmarks = on;
                        cx.notify();
                    },
                    cx,
                ),
            ));

        // The station rows draw nothing off a stream, so they get their own
        // section.
        let live = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(panel::setting_row(
                rox_i18n::t!("seek-lead-in"),
                Some(rox_i18n::t!("seek-lead-in.description")),
                panel::choices_shared(
                    &[
                        (rox_i18n::t!("seek-lead-in-dashed"), LeadIn::Dashed),
                        (rox_i18n::t!("seek-lead-in-faint"), LeadIn::Faint),
                        (rox_i18n::t!("seek-lead-in-hidden"), LeadIn::Hidden),
                    ],
                    self.config.live_lead_in,
                    |this: &mut Self, lead_in, cx| {
                        this.config.live_lead_in = lead_in;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(self.config.live_lead_in == LeadIn::Dashed, |d| {
                d.child(panel::setting_row(
                    rox_i18n::t!("seek-dash-length"),
                    Some(rox_i18n::t!("seek-dash-length.description")),
                    settings_ui::scalar(
                        &self.dash_scrub,
                        &self.value_edit,
                        self.config.live_dash_len,
                        settings_ui::span(DASH_LEN_MIN, DASH_LEN_MAX, " px").hard(),
                        |this: &mut Self, len, cx| {
                            this.config.live_dash_len = len;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .when(self.config.live_lead_in != LeadIn::Hidden, |d| {
                d.child(panel::setting_row(
                    rox_i18n::t!("seek-lead-in-sweep"),
                    Some(rox_i18n::t!("seek-lead-in-sweep.description")),
                    panel::toggle(
                        self.config.live_sweep,
                        |this: &mut Self, on, cx| {
                            this.config.live_sweep = on;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            });

        div()
            .flex()
            .flex_col()
            .gap(settings_ui::SECTION_GAP)
            .child(layout)
            .child(settings_ui::section(
                rox_i18n::t!("seek-section-live"),
                None,
                live,
            ))
            .into_any_element()
    }
}

#[derive(Clone, Copy)]
struct StripLook {
    thickness: f32,
    rounding: f32,
    playhead_width: f32,
    playhead_full: bool,
    playhead_max: f32,
    lead_in: LeadIn,
    dash_len: f32,
    /// The panel's own clock sets it at the call; it isn't copied off the
    /// config.
    sweep: Option<Sweep>,
    /// A paused station's line. Not a config knob, but part of how the line
    /// looks.
    dim: bool,
}

impl From<&SeekConfig> for StripLook {
    fn from(config: &SeekConfig) -> Self {
        StripLook {
            thickness: config.thickness,
            rounding: config.rounding,
            playhead_width: config.playhead_width,
            playhead_full: config.playhead_full,
            playhead_max: config.playhead_max,
            lead_in: config.live_lead_in,
            dash_len: config.live_dash_len,
            sweep: None,
            dim: false,
        }
    }
}

/// `look` holds the line and playhead knobs, the radius capped at a pill.
/// `marker` draws the scrobble threshold, `ab` the repeat section,
/// `marks` the bookmark ribbons along the bottom and `cues` the chevrons
/// along the top.
///
/// A dimmed look halves every alpha for a paused station. Nothing drops
/// out: the tape behind the playhead can still be scrubbed while the
/// station is hung up.
#[allow(clippy::too_many_arguments)]
fn paint_strip(
    progress: f32,
    marker: Option<f32>,
    ab: Option<(f32, Option<f32>)>,
    marks: &[bookmark_ui::Mark],
    cues: &[cue_ui::CueMark],
    look: StripLook,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    let dim = look.dim;
    let head_x = progress.clamp(0.0, 1.0) * w;
    let line_h = look.thickness.clamp(1.0, h);
    let radius = look.rounding.clamp(0.0, line_h / 2.0);
    let line_y = (h - line_h) / 2.0;
    window.paint_quad(
        fill(
            Bounds::new(
                point(bounds.origin.x, bounds.origin.y + px(line_y)),
                size(px(w), px(line_h)),
            ),
            palette::alpha(palette::accent(), if dim { 0x1a } else { 0x33 }),
        )
        .corner_radii(px(radius)),
    );
    window.paint_quad(
        fill(
            Bounds::new(
                point(bounds.origin.x, bounds.origin.y + px(line_y)),
                size(px(head_x), px(line_h)),
            ),
            if dim {
                palette::alpha(palette::accent(), 0x80)
            } else {
                palette::accent()
            },
        )
        // gpui doesn't clamp radii to the quad, so the played side's
        // shrink with it near the start.
        .corner_radii(px(radius.min(head_x / 2.0))),
    );
    if let Some(marker) = marker {
        window.paint_quad(fill(
            Bounds::new(
                point(
                    bounds.origin.x + px(marker.clamp(0.0, 1.0) * w),
                    bounds.origin.y,
                ),
                size(px(1.0), px(h)),
            ),
            palette::alpha(palette::highlight(), 0x80),
        ));
    }
    panel::paint_ab(ab, 1.0, bounds, window);
    bookmark_ui::paint_marks(marks, 1.0, bounds, window);
    cue_ui::paint_marks(cues, 1.0, bounds, window);
    paint_playhead(head_x, look, bounds, window);
}

/// Full height, capped when configured, or the line's height when it
/// hugs; centered either way. The station strip draws the same head.
fn paint_playhead(head_x: f32, look: StripLook, bounds: Bounds<Pixels>, window: &mut Window) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    let line_h = look.thickness.clamp(1.0, h);
    let head_w = look.playhead_width.clamp(1.0, w);
    let head_h = if !look.playhead_full {
        line_h
    } else if look.playhead_max > 0.0 {
        look.playhead_max.clamp(line_h.min(h), h)
    } else {
        h
    };
    window.paint_quad(
        fill(
            Bounds::new(
                point(
                    bounds.origin.x + px(head_x - head_w / 2.0),
                    bounds.origin.y + px((h - head_h) / 2.0),
                ),
                size(px(head_w), px(head_h)),
            ),
            palette::alpha(palette::highlight(), if look.dim { 0x66 } else { 0xd9 }),
        )
        // The playhead reads the config's rounding raw, capped at its own
        // pill: through the line's cap a head fatter than the line could
        // never close into a circle.
        .corner_radii(px(look.rounding.clamp(0.0, head_w.min(head_h) / 2.0))),
    );
}

/// No break is a single row, and an empty side drops its row.
fn split_rows(items: &[SeekItem]) -> Vec<Vec<SeekItem>> {
    items
        .split(|i| matches!(i, SeekItem::Break))
        .filter(|row| !row.is_empty())
        .map(|row| row.to_vec())
        .collect()
}

/// [`split_rows`] with empty rows kept: an added row's well shows until a
/// piece lands or its x drops it.
fn editor_rows(items: &[SeekItem]) -> Vec<Vec<SeekItem>> {
    items
        .split(|i| matches!(i, SeekItem::Break))
        .map(|row| row.to_vec())
        .collect()
}

/// Built once: [`clock`] runs twice per pump tick while playing.
static TNUM: LazyLock<FontFeatures> =
    LazyLock::new(|| FontFeatures(Arc::new(vec![("tnum".into(), 1)])));

/// Tabular digits so a tick never changes the text width.
fn clock(text: String) -> Div {
    let mut clock = div().flex_none().text_color(palette::text_muted());
    clock
        .text_style()
        .get_or_insert_with(Default::default)
        .font_features = Some(TNUM.clone());
    clock.child(text)
}

/// Slow enough to read as waiting: a fast blink is an alarm.
const PULSE_SECS: f32 = 1.6;

/// A sine eased between a dim floor and full, so the mark breathes.
fn pulse(t: f32) -> f32 {
    let phase = t / PULSE_SECS * std::f32::consts::TAU;
    0.45 + 0.55 * (phase.sin() * 0.5 + 0.5)
}

/// Slow enough to read as one band walking across, not a flicker.
const SWEEP_SECS: f32 = 2.2;

/// As a share of the lead-in's length.
const SWEEP_BAND: f32 = 0.25;

/// As a multiple of the lead-in's resting weight.
const SWEEP_GAIN: f32 = 1.6;

/// A quad shades only between its ends, so a wider one would flatten the
/// crest inside it.
const SWEEP_STEP: f32 = 8.0;

/// 0 at the left end of the unfilled stretch, 1 where the tape starts.
#[derive(Clone, Copy)]
struct Sweep(f32);

impl Sweep {
    /// It sets off a band's reach short of the left end and finishes as far
    /// past the right, so the band walks on and off.
    fn at(phase: f32) -> Self {
        let trip = (phase / SWEEP_SECS).rem_euclid(1.0);
        Sweep(-SWEEP_BAND + trip * (1.0 + 2.0 * SWEEP_BAND))
    }

    fn alpha(self, base: u8, along: f32) -> u8 {
        let reach = ((along - self.0).abs() / SWEEP_BAND).min(1.0);
        // A cosine so the band's edges taper instead of ending on a visible line.
        let weight = 0.5 * (1.0 + (reach * std::f32::consts::PI).cos());
        (f32::from(base) * (1.0 + SWEEP_GAIN * weight)).min(255.0) as u8
    }
}

/// Steady rather than breathing: nothing is being waited for.
const PAUSED_OPACITY: f32 = 0.4;

/// Shared with the waveform's corner mark so both read the same. Paused
/// wins: a hung-up station has nothing to wait for, and the accent would
/// say "on air" about a silent strip.
pub(crate) fn live_tint(stream: Option<StreamState>, paused: bool, t: f32) -> (gpui::Rgba, f32) {
    if paused {
        return (palette::text_muted(), PAUSED_OPACITY);
    }

    match stream {
        Some(StreamState::Opening) | Some(StreamState::Reconnecting) => {
            (palette::text_muted(), pulse(t))
        }

        Some(StreamState::Dropped) => (palette::tone_bad(), 1.0),

        // Live, and the moment before the first report lands.
        Some(StreamState::Live) | None => (palette::accent(), 1.0),
    }
}

/// An exact compare: the engine snaps the distance to zero at the edge,
/// and rounding here too would add a second boundary for the clock to
/// flicker across.
fn at_live_edge(behind_secs: f64) -> bool {
    behind_secs <= 0.0
}

/// Shared with the waveform's corner mark.
pub(crate) fn behind_live(shift: Option<&Shift>) -> bool {
    shift.is_some_and(|shift| !at_live_edge(shift.behind_secs))
}

/// The strip spans the whole buffer the setting allows, so the bar keeps
/// its meaning while the tape fills.
fn shift_progress(shift: &Shift) -> f32 {
    if shift.cap_secs <= 0.0 {
        return 1.0;
    }

    // Held to the tape, so a report a hair past the oldest byte can't put
    // the head in the lead-in.
    let behind = shift.behind_secs.clamp(0.0, shift.window_secs);

    (1.0 - behind / shift.cap_secs).clamp(0.0, 1.0) as f32
}

/// The seconds held against the cap, sitting against the right end; the
/// rest is the lead-in.
fn shift_held(shift: &Shift) -> f32 {
    if shift.cap_secs <= 0.0 {
        return 1.0;
    }

    (shift.window_secs / shift.cap_secs).clamp(0.0, 1.0) as f32
}

/// Held to what the tape has, so a grab at the lead-in lands on the
/// oldest thing there is to play.
fn shift_behind(fraction: f32, shift: &Shift) -> f64 {
    let behind = (1.0 - fraction.clamp(0.0, 1.0) as f64) * shift.cap_secs.max(0.0);

    behind.min(shift.window_secs.max(0.0))
}

/// Its own function because it's the whole contract between a press and
/// the mark the insert menu drops.
pub(crate) fn insert_position_ms(fraction: f32, duration_secs: f64) -> u32 {
    let secs = fraction.clamp(0.0, 1.0) as f64 * duration_secs.max(0.0);

    // A track long enough to overflow this is 49 days, so the clamp is
    // for a garbage duration rather than for real music.
    (secs * 1000.0).round().clamp(0.0, u32::MAX as f64) as u32
}

/// Both strips host it. Three orderings hold it together, all of them
/// registration order during paint. The menu builds a frame after the
/// press and never sees the event, so the press stores the position. The
/// press stops, or the dock's body handler opens the panel dropdown over
/// the menu. And the layer goes in before the mark overlays, so a right
/// click on a chevron reaches that mark's own menu instead.
///
/// Stopping the press is why the panel's dropdown is appended here:
/// otherwise a right click on the strip would put the panel's settings
/// out of reach.
pub(crate) fn insert_layer<V: Panel>(
    state: &AppState,
    key: &TrackKey,
    duration_secs: f64,
    scrub: &ScrubState,
    at_ms: &Arc<AtomicU32>,
    cx: &mut Context<V>,
) -> impl IntoElement + use<V> {
    let press_scrub = scrub.clone();
    let press_at = at_ms.clone();
    let menu_state = state.clone();
    let menu_key = key.clone();
    let menu_at = at_ms.clone();
    let menu_panel = cx.entity().downgrade();

    div()
        .absolute()
        .inset_0()
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |_, event: &gpui::MouseDownEvent, _, cx| {
                if let Some(fraction) = press_scrub.fraction(event.position.x) {
                    press_at.store(
                        insert_position_ms(fraction, duration_secs),
                        Ordering::Relaxed,
                    );
                }
                cx.stop_propagation();
            }),
        )
        .context_menu(move |menu, window, cx| {
            let menu = cue_ui::insert_menu(
                menu,
                menu_state.clone(),
                menu_key.clone(),
                menu_at.load(Ordering::Relaxed),
                window,
                cx,
            );

            let Some(panel) = menu_panel.upgrade() else {
                return menu;
            };

            panel.update(cx, |panel, cx| {
                panel.dropdown_menu(menu.separator(), window, cx)
            })
        })
}

/// Everything that arrives is drawn: the tape already clips the list to
/// what it holds, and a second check here would only be a second place
/// to disagree. The id is the index.
fn tape_marks(songs: &[LiveMark], shift: &Shift) -> Vec<cue_ui::CueMark> {
    if shift.cap_secs <= 0.0 {
        return Vec::new();
    }

    songs
        .iter()
        .enumerate()
        .map(|(i, song)| cue_ui::CueMark {
            id: i as u64,
            fraction: (1.0 - song.behind_secs / shift.cap_secs).clamp(0.0, 1.0) as f32,
            // No time of its own: the readout is the song's name, and the click
            // seeks by the mark's distance.
            position_ms: 0,
        })
        .collect()
}

/// A fraction only: a break is the one place a seek won't go, so it just
/// has to be visible.
fn tape_gaps(gaps: &[LiveGap], shift: &Shift) -> Vec<f32> {
    if shift.cap_secs <= 0.0 {
        return Vec::new();
    }

    gaps.iter()
        .map(|gap| (1.0 - gap.behind_secs / shift.cap_secs).clamp(0.0, 1.0) as f32)
        .collect()
}

/// None at the edge, where the LIVE mark stands. Floored, so it ticks
/// once a second like a countdown.
fn behind_clock(shift: Option<&Shift>, digits: usize) -> Option<String> {
    let shift = shift.filter(|shift| !at_live_edge(shift.behind_secs))?;

    Some(format!(
        "-{}",
        fmt_time_padded(shift.behind_secs.floor(), digits)
    ))
}

/// Back in the tape the mark drops the accent and becomes the way
/// forward. A reconnect or a drop still wins.
pub(crate) fn live_mark_tint(
    stream: Option<StreamState>,
    paused: bool,
    behind: bool,
    t: f32,
) -> (gpui::Rgba, f32) {
    let on_air = !paused && matches!(stream, Some(StreamState::Live) | None);
    if behind && on_air {
        return (palette::text_muted(), 1.0);
    }

    live_tint(stream, paused, t)
}

/// The clock's shape, so row widths don't jump between a file and a
/// stream. The accent while audio arrives, muted and breathing while
/// waiting, the bad tone once reconnects run out; the non-playing states
/// carry a tooltip.
///
/// Also the button back to the live edge. It stays a button at the edge,
/// where the click does nothing: one that only appears once it works is
/// one nobody knows about. Two slots can draw it, so each passes its own
/// id.
fn live_mark(
    id: &'static str,
    stream: Option<StreamState>,
    paused: bool,
    behind: bool,
    t: f32,
    player: &Entity<Player>,
) -> AnyElement {
    let (color, opacity) = live_mark_tint(stream, paused, behind, t);
    let player = player.clone();
    let mark = clock(rox_i18n::t!("transport-live").to_string())
        .text_color(color)
        .opacity(opacity)
        .id(id)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            player.read(cx).go_live();
        });
    let tip = match stream {
        Some(StreamState::Opening) => Some(rox_i18n::t!("transport-live-opening")),
        Some(StreamState::Reconnecting) => Some(rox_i18n::t!("transport-live-reconnecting")),
        Some(StreamState::Dropped) => Some(rox_i18n::t!("transport-live-dropped")),
        Some(StreamState::Live) | None => behind.then(|| rox_i18n::t!("transport-live-jump")),
    };

    match tip {
        Some(tip) => mark
            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
            .into_any_element(),

        None => mark.into_any_element(),
    }
}

/// On the stock row this slot is the only live control, so the distance
/// is also the way back: the minus sign says where you are and the click
/// undoes it.
fn behind_mark(text: String, player: &Entity<Player>) -> AnyElement {
    let tip = rox_i18n::t!("transport-live-jump");
    let player = player.clone();

    clock(text)
        .id("transport-behind-clock")
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
            player.read(cx).go_live();
        })
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .into_any_element()
}

/// A station sending one unsplittable field leaves the artist empty. The
/// stations panel spells this the same way; neither module owns the
/// other.
fn song_text(artist: &str, title: &str) -> String {
    if artist.is_empty() {
        return title.to_string();
    }

    format!("{artist} - {title}")
}

/// [`cue_ui::overlay`]'s shape, with the song's name in the readout and
/// none of the editing: these marks are the station's.
fn song_overlay(
    songs: &[LiveMark],
    marks: &[cue_ui::CueMark],
    hovered: Option<u64>,
    scrub: &ScrubState,
    player: &Entity<Player>,
    cx: &mut Context<SeekStripPanel>,
) -> Div {
    let mut layer = div().absolute().inset_0();

    for mark in marks {
        let id = mark.id;
        let Some(song) = songs.get(id as usize) else {
            continue;
        };
        let behind_secs = song.behind_secs;
        let player = player.clone();
        let hover_scrub = scrub.clone();
        let hit = div()
            .id(("song-mark", id))
            .size_full()
            .cursor_pointer()
            // Clears the strip's own preview so the song readout is the only one.
            // The pointer's place is kept for `settle_song_hover`.
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                hover_scrub.set_hover(None);
                this.pointer_at = hover_scrub.fraction(event.position.x);
                cx.stop_propagation();
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.hovered_song = hovered.then_some(id);
                cx.notify();
            }))
            // Seeks to the song's own mark, not the pixel, and the strip's seek
            // stays out of it.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, _: &gpui::MouseDownEvent, _, cx| {
                    player.read(cx).seek_live(behind_secs);
                    cx.stop_propagation();
                }),
            );

        // The top half only, matching where the chevrons hang.
        layer = layer.child(
            div()
                .id(("song-slot", id))
                .absolute()
                .top_0()
                .bottom(relative(0.5))
                .left(relative(mark.fraction))
                .w(px(SONG_HIT_W))
                .ml(px(-SONG_HIT_W / 2.0))
                .child(hit),
        );
    }

    if let Some((mark, song)) = hovered.and_then(|id| {
        marks
            .iter()
            .find(|m| m.id == id)
            .zip(songs.get(id as usize))
    }) {
        layer = layer.child(song_readout(
            mark.fraction,
            song_text(&song.artist, &song.title),
        ));
    }

    layer
}

/// Wider than the drawing so a pointer finds it; the cue layer's width,
/// since both draw the same chevron.
const SONG_HIT_W: f32 = 16.0;

/// Under the chevron, which already sits on the top edge.
fn song_readout(fraction: f32, text: String) -> Div {
    div()
        .absolute()
        .bottom(tokens::SPACE_XS)
        .left(relative(fraction))
        .w_0()
        .flex()
        .flex_col()
        .items_center()
        .child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .px(tokens::SPACE_SM)
                .py(px(2.))
                .rounded(tokens::RADIUS)
                .bg(palette::bg_menu_opaque())
                .border_1()
                .border_color(palette::border())
                .text_sm()
                .text_color(palette::text())
                .child(text),
        )
}

/// Held to what the tape has, so the pill over the lead-in stops at the
/// oldest second. [`panel::seek_hover`]'s shape with the tape's clock,
/// since a broadcast has no absolute time.
fn shift_hover(
    scrub: &ScrubState,
    shift: Shift,
    cx: &mut Context<SeekStripPanel>,
) -> Stateful<Div> {
    let moved = scrub.clone();
    let left = scrub.clone();
    let hover = scrub.hover();
    div()
        .id("shift-hover")
        .absolute()
        .inset_0()
        .cursor_pointer()
        .on_mouse_move(cx.listener(move |_, event: &MouseMoveEvent, _, cx| {
            if moved.set_hover(moved.fraction(event.position.x)) {
                cx.notify();
            }
        }))
        // The leave stops the moves, so it has to clear the readout itself.
        .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
            if !hovered && left.set_hover(None) {
                cx.notify();
            }
        }))
        .when_some(hover, |d, fraction| {
            d.child(shift_pill(fraction, shift_behind(fraction, &shift)))
        })
}

/// The right end says LIVE rather than -0:00.
fn shift_pill(fraction: f32, behind_secs: f64) -> Div {
    let text = if at_live_edge(behind_secs) {
        rox_i18n::t!("transport-live").to_string()
    } else {
        format!("-{}", fmt_time(behind_secs))
    };

    div()
        .absolute()
        .top(tokens::SPACE_XS)
        .left(relative(fraction))
        .w_0()
        .flex()
        .flex_col()
        .items_center()
        .child(
            div()
                .flex_none()
                // The zero-width column gives the text no room, so without this it
                // wraps one glyph per line.
                .whitespace_nowrap()
                .px(tokens::SPACE_SM)
                .py(px(2.))
                .rounded(tokens::RADIUS)
                .bg(palette::bg_menu_opaque())
                .border_1()
                .border_color(palette::border())
                .text_sm()
                .text_color(palette::text())
                .child(text),
        )
}

/// The sweep measures against the whole span, so every dash shades off
/// the same band.
#[derive(Clone, Copy)]
struct LeadLine {
    /// The strip's left edge and the line's top, in window space.
    left: Pixels,
    top: Pixels,
    height: f32,
    radius: f32,
    span: f32,
    sweep: Option<Sweep>,
}

impl LeadLine {
    /// `x` and `width` are px from the strip's left edge; `alpha` is the
    /// resting weight with no band on it.
    fn piece(self, x: f32, width: f32, alpha: u8, window: &mut Window) {
        let radius = self.radius.min(width / 2.0);
        let quad = |x: f32,
                    width: f32,
                    paint: Background,
                    corners: Corners<Pixels>,
                    window: &mut Window| {
            window.paint_quad(
                fill(
                    Bounds::new(
                        point(self.left + px(x), self.top),
                        size(px(width), px(self.height)),
                    ),
                    paint,
                )
                .corner_radii(corners),
            );
        };

        let Some(sweep) = self.sweep else {
            let flat = palette::alpha(palette::accent(), alpha);
            quad(x, width, flat.into(), Corners::all(px(radius)), window);
            return;
        };

        // A quad shades only between its ends, so a wide stretch is cut up for
        // the crest to sit inside it. The cuts keep the outer corners rounded
        // and the seams square, so a dash still reads as one.
        let cuts = (width / SWEEP_STEP).ceil().max(1.0);
        let step = width / cuts;
        let cuts = cuts as usize;
        let span = self.span.max(1.0);
        for cut in 0..cuts {
            let from = x + step * cut as f32;
            let shade = |at: f32| palette::alpha(palette::accent(), sweep.alpha(alpha, at / span));
            let end = |on: bool| px(if on { radius } else { 0.0 });
            let corners = Corners {
                top_left: end(cut == 0),
                bottom_left: end(cut == 0),
                top_right: end(cut + 1 == cuts),
                bottom_right: end(cut + 1 == cuts),
            };
            quad(
                from,
                step,
                linear_gradient(
                    90.,
                    linear_color_stop(shade(from), 0.0),
                    linear_color_stop(shade(from + step), 1.0),
                ),
                corners,
                window,
            );
        }
    }
}

/// Narrow on purpose: it marks a splice the listener can't cross, not a
/// stretch of silence.
const GAP_BREAK_W: f32 = 5.0;
const GAP_LINE_W: f32 = 1.5;
/// Enough to catch the eye on a three-pixel bar. The height is
/// crate-public because the waveform leaves the notch room above its
/// strip.
const GAP_NOTCH_W: f32 = 5.0;
pub(crate) const GAP_NOTCH_H: f32 = 3.0;
const GAP_ALPHA: u8 = 0xcc;

/// A gap goes through the middle, touching neither edge: cues own the
/// top and bookmarks the bottom. Muted rather than the accent, since
/// nobody put it there.
///
/// `band` is the cut's top and height in strip-local px; `weight` scales
/// the alpha for a dimmed strip.
pub(crate) fn paint_gaps(
    gaps: &[f32],
    band: (f32, f32),
    weight: f32,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let w = f32::from(bounds.size.width);
    let (top, height) = band;
    if w <= 0.0 || height <= 0.0 || gaps.is_empty() {
        return;
    }

    let alpha = (GAP_ALPHA as f32 * weight.clamp(0.0, 1.0)) as u8;
    if alpha == 0 {
        return;
    }

    let color = palette::alpha(palette::text_muted(), alpha);
    // A strip too short for the full notch gets a shorter one.
    let notch_h = GAP_NOTCH_H.min(top);
    let quad = |x: f32, w: f32, y: f32, h: f32, color, window: &mut Window| {
        window.paint_quad(fill(
            Bounds::new(
                point(bounds.origin.x + px(x), bounds.origin.y + px(y)),
                size(px(w), px(h)),
            ),
            color,
        ));
    };

    for gap in gaps {
        let x = gap.clamp(0.0, 1.0) * w;

        // The hole first, in the panel's background, so the bar reads as
        // stopping rather than carrying on under a mark.
        quad(
            x - GAP_BREAK_W / 2.0,
            GAP_BREAK_W,
            top,
            height,
            palette::bg_root(),
            window,
        );
        quad(x - GAP_LINE_W / 2.0, GAP_LINE_W, top, height, color, window);

        if notch_h > 0.0 {
            quad(
                x - GAP_NOTCH_W / 2.0,
                GAP_NOTCH_W,
                top - notch_h,
                notch_h,
                color,
                window,
            );
        }
    }
}

/// The dashes on the left are buffer the setting allows and the
/// connection hasn't filled. A fresh station fills in leftwards, so the
/// bar reads the same from the first second. Nothing there is reachable.
fn paint_shift_strip(
    shift: &Shift,
    songs: &[cue_ui::CueMark],
    gaps: &[f32],
    look: StripLook,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    let line_h = look.thickness.clamp(1.0, h);
    let radius = look.rounding.clamp(0.0, line_h / 2.0);
    let line_y = bounds.origin.y + px((h - line_h) / 2.0);
    let wash_alpha = if look.dim { 0x1a } else { 0x33 };
    let wash = palette::alpha(palette::accent(), wash_alpha);
    let tape_x = (1.0 - shift_held(shift)) * w;
    let head_x = (shift_progress(shift) * w).max(tape_x);

    // The dash's gap matches its length, so one number sets the texture.
    let line = LeadLine {
        left: bounds.origin.x,
        top: line_y,
        height: line_h,
        radius,
        span: tape_x,
        sweep: look.sweep,
    };
    let lead = |dash: f32, alpha: u8, window: &mut Window| {
        let step = dash * 2.0;
        let mut x = 0.0;
        while x < tape_x {
            line.piece(x, dash.min(tape_x - x), alpha, window);
            x += step;
        }
    };
    match look.lead_in {
        LeadIn::Dashed => lead(
            look.dash_len.clamp(DASH_LEN_MIN, DASH_LEN_MAX),
            wash_alpha,
            window,
        ),

        // Half the tape's weight so the two still read apart.
        LeadIn::Faint => lead(tape_x, if look.dim { 0x0d } else { 0x1a }, window),

        LeadIn::Hidden => {}
    }

    window.paint_quad(
        fill(
            Bounds::new(
                point(bounds.origin.x + px(tape_x), line_y),
                size(px(w - tape_x), px(line_h)),
            ),
            wash,
        )
        .corner_radii(px(radius)),
    );
    let heard = head_x - tape_x;
    window.paint_quad(
        fill(
            Bounds::new(
                point(bounds.origin.x + px(tape_x), line_y),
                size(px(heard), px(line_h)),
            ),
            if look.dim {
                palette::alpha(palette::accent(), 0x80)
            } else {
                palette::accent()
            },
        )
        // gpui doesn't clamp radii to the quad, so the heard side's shrink
        // with it near the tape's start.
        .corner_radii(px(radius.min(heard / 2.0))),
    );

    // After both fills, since a break cuts whichever covers it, and before
    // the playhead, which crosses it.
    paint_gaps(
        gaps,
        (f32::from(line_y - bounds.origin.y), line_h),
        if look.dim { 0.5 } else { 1.0 },
        bounds,
        window,
    );

    // Off the top edge, the same chevrons a track's cues get.
    cue_ui::paint_marks(songs, 1.0, bounds, window);
    paint_playhead(head_x, look, bounds, window);
}

/// The line at full width with nothing moving along it, so the row keeps
/// its shape through a stream's first seconds.
fn paint_live_strip(look: StripLook, bounds: Bounds<Pixels>, window: &mut Window) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    // Dims with the marks on a paused station.
    let alpha = if look.dim { 0x2a } else { 0x66 };
    let line_h = look.thickness.clamp(1.0, h);
    let radius = look.rounding.clamp(0.0, line_h / 2.0);
    window.paint_quad(
        fill(
            Bounds::new(
                point(bounds.origin.x, bounds.origin.y + px((h - line_h) / 2.0)),
                size(px(w), px(line_h)),
            ),
            palette::alpha(palette::accent(), alpha),
        )
        .corner_radii(px(radius)),
    );
}

impl Render for SeekStripPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(window, cx).track_focus(&focus))
    }
}

impl SeekStripPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let player = self.state.player.read(cx);
        let now = player.now_playing();
        let ab = player.ab_state();

        // No frame polling: the raw observe in `new` re-renders on every pump
        // tick, the rate the clock and playhead change at. A per-frame request
        // would keep the window repainting through a paused session.

        let root = div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .flex_col()
            .justify_center();

        let Some(now) = now else {
            // Idle until a session brings a track.
            return root;
        };

        // A station has no timeline, so everything drawn against a track's
        // length drops out rather than drawing against a zero.
        let live = now.live;
        // None off a station, and for its first seconds before the tape holds
        // a byte.
        let shift = now.shift;
        // A stream waiting to open moves nothing else, so the pulse asks for
        // its own frames.
        let stream = now.stream;
        // A paused station is hung up: the marks dim and nothing breathes.
        let paused = live && !player.is_playing();
        let waiting = live
            && !paused
            && matches!(
                stream,
                Some(StreamState::Opening) | Some(StreamState::Reconnecting)
            );
        // The lead-in band moves on the clock, so it asks for frames. Not while
        // paused, and not without a lead-in (a full or unstarted tape): a
        // station left on overnight shouldn't hold the window at refresh rate.
        let sweeping = live
            && !paused
            && self.config.live_sweep
            && self.config.live_lead_in != LeadIn::Hidden
            && shift.as_ref().is_some_and(|shift| shift_held(shift) < 1.0);
        if waiting || sweeping {
            window.request_animation_frame();
        }
        let phase = self.epoch.elapsed().as_secs_f32();

        let progress = now
            .duration_secs
            .filter(|d| *d > 0.0)
            .map(|d| (now.position_secs / d) as f32)
            .unwrap_or(0.0);
        // Only where a scrobble could happen: the toggle on and a destination
        // armed.
        let marker = (!live && self.config.scrobble_marker)
            .then(|| self.state.scrobble_marker(cx))
            .flatten();
        // The A-B section, or the lone A while the cycle waits for B.
        let ab = (!live)
            .then(|| panel::ab_fractions(ab, now.duration_secs))
            .flatten();
        // Everything keyed to a track position drops out together on a station:
        // the marks, the insert layer, and their menus.
        let positional = !live && position_bound::allowed(&self.state, cx);
        let marks = if self.config.bookmarks && positional {
            bookmark_ui::marks(self.marks_for(&now.key, cx), now.duration_secs)
        } else {
            Vec::new()
        };
        let hover_mark = self.hover_mark;
        let cues = if positional {
            cue_ui::marks(self.cues_for(&now.key, cx), now.duration_secs)
        } else {
            Vec::new()
        };
        let hovered_cue = self.hovered_cue;
        // Read every paint, not cached on the title revision: each mark's
        // distance from the live edge grows with the broadcast.
        let songs = if shift.is_some() {
            self.state.player.read(cx).live_marks()
        } else {
            Vec::new()
        };
        let song_marks = shift
            .as_ref()
            .map(|shift| tape_marks(&songs, shift))
            .unwrap_or_default();
        self.settle_song_hover(&song_marks);
        let hovered_song = self.hovered_song;
        // Mapped on the same tick as the songs, or the two would drift apart.
        let gap_marks = shift
            .as_ref()
            .map(|shift| tape_gaps(&self.state.player.read(cx).live_gaps(), shift))
            .unwrap_or_default();
        // The seek click is on the track alone, so the clocks stay inert. The
        // preview waits for the duration.
        let hover_duration = now.duration_secs.filter(|d| *d > 0.0 && !live);
        // A station maps once it has a tape: the strip is the buffer then.
        let seekable = !live || shift.is_some();
        // Waits for the duration, like the seek preview.
        let insert = now
            .duration_secs
            .filter(|_| positional)
            .filter(|d| *d > 0.0)
            .map(|duration| {
                insert_layer(
                    &self.state,
                    &now.key,
                    duration,
                    &self.scrub,
                    &self.insert_at_ms,
                    cx,
                )
            });
        let scrub = self.scrub.clone();
        let player = self.state.player.clone();
        let look = StripLook {
            dim: paused,
            sweep: sweeping.then(|| Sweep::at(phase)),
            ..StripLook::from(&self.config)
        };
        let track = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .when(seekable, |d| {
                d.cursor_pointer().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                        this.scrub.begin();
                        if let Some(fraction) = this.scrub.fraction(event.position.x) {
                            this.seek_at(fraction, cx);
                        }
                        cx.notify();
                    }),
                )
            })
            // Before the canvas and both overlays, which is what puts this
            // last in line for a right click. See `insert_layer`.
            .children(insert)
            .child(
                canvas(
                    {
                        let scrub = scrub.clone();
                        move |bounds, _, _| scrub.set_bounds(bounds)
                    },
                    {
                        let marks = marks.clone();
                        let cues = cues.clone();
                        let songs = song_marks.clone();
                        let gaps = gap_marks.clone();
                        move |bounds, _, window, _| {
                            // Nothing held yet: the flat bar, and no drag to arm.
                            if live && shift.is_none() {
                                paint_live_strip(look, bounds, window);
                                return;
                            }

                            match &shift {
                                // The buffer, with the station's songs and breaks on it. No track
                                // marks: nothing has a position on a broadcast.
                                Some(shift) => {
                                    paint_shift_strip(shift, &songs, &gaps, look, bounds, window)
                                }

                                None => paint_strip(
                                    progress, marker, ab, &marks, &cues, look, bounds, window,
                                ),
                            }

                            panel::scrub_on_paint(&scrub, window, {
                                let player = player.clone();
                                move |fraction, cx| match &shift {
                                    Some(shift) => {
                                        player.read(cx).seek_live(shift_behind(fraction, shift))
                                    }

                                    None => panel::seek_fraction(&player, fraction, cx),
                                }
                            });
                        }
                    },
                )
                .size_full(),
            )
            .when_some(hover_duration, |d, duration| {
                d.child(panel::seek_hover(&self.scrub, duration, cx))
            })
            // Reads back from the live edge rather than forward from a track's
            // start.
            .when_some(shift, |d, shift| {
                d.child(shift_hover(&self.scrub, shift, cx))
            })
            // Over the seek readout's layer, so a pointer on a mark reads the mark.
            .when(!marks.is_empty(), |d| {
                d.child(bookmark_ui::overlay(
                    &self.state,
                    &now.key,
                    &marks,
                    hover_mark,
                    &self.scrub,
                    |this: &mut Self, id, _| this.hover_mark = id,
                    cx,
                ))
            })
            .when(!cues.is_empty(), |d| {
                d.child(cue_ui::overlay(
                    &self.state,
                    &now.key,
                    &cues,
                    hovered_cue,
                    &self.scrub,
                    |this: &mut Self, id, _| this.hovered_cue = id,
                    cx,
                ))
            })
            // On the cues' edge, which a station never uses.
            .when(!song_marks.is_empty(), |d| {
                d.child(song_overlay(
                    &songs,
                    &song_marks,
                    hovered_song,
                    &self.scrub,
                    &self.state.player,
                    cx,
                ))
            });

        // Minutes pad to the duration's digits so neither clock changes width
        // mid-track.
        let digits = now
            .duration_secs
            .map(|d| (d as u64 / 60).to_string().len())
            .unwrap_or(1);
        // A station's clock shows the song, going back to zero when the stream
        // says the next one started.
        let elapsed = song_clock(now.position_secs, now.song_start_secs);
        let ending = match now.duration_secs {
            Some(d) if self.config.show_total => fmt_time_padded(d, digits),
            Some(d) => format!(
                "-{}",
                fmt_time_padded((d - now.position_secs).max(0.0), digits)
            ),
            None => "-:--".into(),
        };
        // Counts back to the live edge. None at the edge, where the LIVE mark
        // has the slot.
        let behind = behind_clock(shift.as_ref(), digits);

        // The strip's row takes whatever height the others leave, so a stacked
        // layout keeps the strip broad.
        let mut track = Some(track);
        let mut piece = |item: &SeekItem| -> Option<AnyElement> {
            match item {
                SeekItem::Elapsed => {
                    Some(clock(fmt_time_padded(elapsed, digits)).into_any_element())
                }
                SeekItem::Strip => track.take().map(|t| t.into_any_element()),
                // LIVE at the edge, since a countdown to an end that never comes is
                // worse than none, then the distance back. Either face is the button
                // back to live.
                SeekItem::Ending if live => Some(match &behind {
                    Some(behind) => behind_mark(behind.clone(), &self.state.player),

                    None => live_mark(
                        "transport-live-ending",
                        stream,
                        paused,
                        false,
                        phase,
                        &self.state.player,
                    ),
                }),
                // The duration slot keeps the mark regardless, so a row with both has
                // the way back beside the distance.
                SeekItem::Duration if live => Some(live_mark(
                    "transport-live-duration",
                    stream,
                    paused,
                    behind.is_some(),
                    phase,
                    &self.state.player,
                )),
                SeekItem::Ending => Some(clock(ending.clone()).into_any_element()),
                SeekItem::Duration => Some(
                    clock(match now.duration_secs {
                        Some(d) => fmt_time_padded(d, digits),
                        None => "-:--".into(),
                    })
                    .into_any_element(),
                ),
                SeekItem::Spacer => Some(div().flex_1().into_any_element()),
                SeekItem::Divider => Some(
                    div()
                        .flex_1()
                        .h(px(1.))
                        .bg(palette::border())
                        .into_any_element(),
                ),
                SeekItem::Break => None,
            }
        };
        let rows: Vec<Div> = split_rows(&self.config.items)
            .into_iter()
            .map(|items| {
                // Any clock brings its row's padding in; a strip-only row runs edge to
                // edge.
                let has_clock = items.iter().any(|i| {
                    matches!(i, SeekItem::Elapsed | SeekItem::Ending | SeekItem::Duration)
                });
                let stretch = items.contains(&SeekItem::Strip);
                div()
                    .flex()
                    .items_center()
                    .w_full()
                    .map(|d| {
                        if stretch {
                            d.flex_1().min_h_0()
                        } else {
                            d.flex_none()
                        }
                    })
                    .when(has_clock, |d| d.gap(tokens::SPACE_SM).px(tokens::SPACE_SM))
                    .children(items.iter().filter_map(&mut piece))
            })
            .collect();
        root.children(rows)
    }
}

// The width is the seek strip's clocks around a usable track.
transport_panel!(
    SeekStripPanel,
    "seek",
    rox_i18n::t!("panel-title-seek"),
    min_w = 160.
);

#[cfg(test)]
mod tests {
    use super::{
        LeadIn, LiveMark, SeekConfig, SeekItem, Shift, Sweep, behind_clock, editor_rows,
        insert_position_ms, shift_behind, shift_held, shift_progress, split_rows, tape_marks,
    };

    fn tape(behind_secs: f64, window_secs: f64, cap_secs: f64) -> Shift {
        Shift {
            behind_secs,
            window_secs,
            cap_secs,
            bytes_per_sec: 16_000.0,
            song_secs: None,
            song_len_secs: None,
        }
    }

    #[test]
    fn a_right_click_lands_at_its_fraction_of_the_track() {
        assert_eq!(insert_position_ms(0.0, 120.0), 0);
        assert_eq!(insert_position_ms(0.5, 120.0), 60_000);
        assert_eq!(insert_position_ms(1.0, 120.0), 120_000);

        // Rounds rather than truncating, so the mark sits where the pointer was.
        assert_eq!(insert_position_ms(0.5, 0.001), 1);

        assert_eq!(insert_position_ms(-0.5, 120.0), 0);
        assert_eq!(insert_position_ms(1.5, 120.0), 120_000);
        assert_eq!(insert_position_ms(0.5, 0.0), 0);
        assert_eq!(insert_position_ms(0.5, -30.0), 0);

        assert_eq!(insert_position_ms(1.0, 1.0e12), u32::MAX);
    }

    #[test]
    fn the_playhead_sits_where_the_tape_has_been_heard() {
        assert!(shift_progress(&tape(0.0, 600.0, 600.0)) == 1.0);
        assert!(shift_progress(&tape(150.0, 600.0, 600.0)) == 0.75);
        assert!(shift_progress(&tape(600.0, 600.0, 600.0)) == 0.0);

        // Half a buffer taped: the head stops at the middle, the oldest second
        // held.
        assert!(shift_progress(&tape(0.0, 300.0, 600.0)) == 1.0);
        assert!(shift_progress(&tape(300.0, 300.0, 600.0)) == 0.5);
        assert!(shift_progress(&tape(400.0, 300.0, 600.0)) == 0.5);

        assert!(shift_progress(&tape(0.0, 0.0, 0.0)) == 1.0);
    }

    #[test]
    fn the_tape_fills_the_bar_rather_than_growing_it() {
        assert!(shift_held(&tape(0.0, 0.0, 600.0)) == 0.0);
        assert!(shift_held(&tape(0.0, 300.0, 600.0)) == 0.5);
        assert!(shift_held(&tape(0.0, 600.0, 600.0)) == 1.0);
        assert!(shift_held(&tape(0.0, 0.0, 0.0)) == 1.0);
    }

    #[test]
    fn a_fraction_maps_back_to_a_distance_from_live() {
        let full = tape(0.0, 600.0, 600.0);
        assert!(shift_behind(1.0, &full) == 0.0);
        assert!(shift_behind(0.5, &full) == 300.0);
        assert!(shift_behind(0.0, &full) == 600.0);

        assert!(shift_behind(1.5, &full) == 0.0);
        assert!(shift_behind(-0.5, &full) == 600.0);

        let half = tape(0.0, 300.0, 600.0);
        assert!(shift_behind(0.5, &half) == 300.0);
        assert!(shift_behind(0.25, &half) == 300.0);
        assert!(shift_behind(0.0, &half) == 300.0);
    }

    #[test]
    fn the_ending_slot_counts_back_only_once_it_is_behind() {
        assert!(behind_clock(None, 1).is_none());
        assert!(behind_clock(Some(&tape(0.0, 600.0, 600.0)), 1).is_none());

        assert!(behind_clock(Some(&tape(0.4, 600.0, 600.0)), 1).as_deref() == Some("-0:00"));
        // Floored, so a value on a half second can't tick between two readings.
        assert!(behind_clock(Some(&tape(65.9, 600.0, 600.0)), 1).as_deref() == Some("-1:05"));
        assert!(behind_clock(Some(&tape(65.0, 600.0, 600.0)), 2).as_deref() == Some("-01:05"));
    }

    fn song(behind_secs: f64) -> LiveMark {
        LiveMark {
            behind_secs,
            artist: "Artist".into(),
            title: "Title".into(),
        }
    }

    #[test]
    fn song_marks_sit_where_the_tape_put_them() {
        let shift = tape(0.0, 300.0, 600.0);
        let marks = tape_marks(&[song(0.0), song(150.0), song(300.0)], &shift);
        assert!(marks.len() == 3);
        assert!(marks[0].fraction == 1.0);
        assert!(marks[1].fraction == 0.75);
        assert!(marks[2].fraction == 0.5);
        // The ids are list indexes, not engine ids.
        assert!(marks[2].id == 2);

        assert!(tape_marks(&[song(0.0)], &tape(0.0, 0.0, 0.0)).is_empty());
    }

    #[test]
    fn the_lead_in_defaults_to_dashes_and_round_trips() {
        let config: SeekConfig = serde_json::from_str("{}").unwrap();
        assert!(config.live_lead_in == LeadIn::Dashed);
        assert!(config.live_dash_len == super::DASH_LEN_DEFAULT);
        assert!(!config.live_sweep);

        let config: SeekConfig = serde_json::from_str(
            r#"{"live_lead_in": "hidden", "live_dash_len": 9.0, "live_sweep": true}"#,
        )
        .unwrap();
        assert!(config.live_lead_in == LeadIn::Hidden);
        assert!(config.live_dash_len == 9.0);
        assert!(config.live_sweep);

        let saved = serde_json::to_value(&config).unwrap();
        let back: SeekConfig = serde_json::from_value(saved).unwrap();
        assert!(back.live_lead_in == LeadIn::Hidden);
        assert!(back.live_dash_len == 9.0);
        assert!(back.live_sweep);
    }

    #[test]
    fn the_sweep_walks_the_lead_in_and_starts_over() {
        let base = 0x33;
        let start = Sweep::at(0.0);
        assert!(start.alpha(base, 0.5) == base);
        assert!(start.alpha(base, 1.0) == base);
        let middle = Sweep::at(super::SWEEP_SECS / 2.0);
        assert!(middle.alpha(base, 0.5) > base);
        assert!(middle.alpha(base, 0.0) == base);
        let crest = f32::from(base) * (1.0 + super::SWEEP_GAIN);
        assert!(middle.alpha(base, 0.5) == crest as u8);
        assert!(Sweep::at(super::SWEEP_SECS).alpha(base, 0.5) == start.alpha(base, 0.5));
    }

    #[test]
    fn legacy_timings_folds_into_the_item_list() {
        let config: SeekConfig = serde_json::from_str("{}").unwrap();
        assert!(config.items == SeekConfig::default().items);

        let config: SeekConfig = serde_json::from_str(r#"{"timings": false}"#).unwrap();
        assert!(config.items == vec![SeekItem::Strip]);
    }

    #[test]
    fn item_lists_read_ordered_and_deduped() {
        let config: SeekConfig =
            serde_json::from_str(r#"{"items": ["strip", "elapsed", "strip"]}"#).unwrap();
        assert!(config.items == vec![SeekItem::Strip, SeekItem::Elapsed]);

        // Uniqueness is per row: a copy across a break survives the load.
        let config: SeekConfig =
            serde_json::from_str(r#"{"items": ["elapsed", "break", "elapsed"]}"#).unwrap();
        assert!(config.items == vec![SeekItem::Elapsed, SeekItem::Break, SeekItem::Elapsed]);

        let saved = serde_json::to_value(&config).unwrap();
        let back: SeekConfig = serde_json::from_value(saved).unwrap();
        assert!(back.items == config.items);
    }

    #[test]
    fn break_cuts_the_list_into_rows() {
        let config: SeekConfig =
            serde_json::from_str(r#"{"items": ["strip", "break", "elapsed", "ending"]}"#).unwrap();
        let rows = split_rows(&config.items);
        assert!(
            rows == vec![
                vec![SeekItem::Strip],
                vec![SeekItem::Elapsed, SeekItem::Ending]
            ]
        );

        let rows = split_rows(&[SeekItem::Break, SeekItem::Strip]);
        assert!(rows == vec![vec![SeekItem::Strip]]);

        let rows = split_rows(&SeekConfig::default().items);
        assert!(rows == vec![SeekConfig::default().items]);
    }

    #[test]
    fn editor_rows_keep_empties_and_rejoin() {
        let items = vec![SeekItem::Strip, SeekItem::Break];
        let rows = editor_rows(&items);
        assert!(rows == vec![vec![SeekItem::Strip], vec![]]);
        assert!(rows.join(&SeekItem::Break) == items);
    }
}
