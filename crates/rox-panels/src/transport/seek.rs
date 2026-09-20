//! The seek strip panel: a track line with the played side in the accent
//! and a playhead, click or drag to seek, the elapsed and remaining clocks
//! at its ends.
//!
//! A station draws on the same line once the engine holds a timeshift tape
//! for it. The strip spans the whole buffer the setting allows, the live
//! edge is its right end, and a click lands in the past rather than at a
//! position. What hasn't taped yet is a dashed lead-in on the left and
//! can't be reached. Everything keyed to a track position (bookmarks, the
//! scrobble threshold, A-B) stays off there, since a broadcast has no such
//! position.

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

/// One piece of the seek row, the arrange editor's unit. The config's
/// list holds the shown ones in display order.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SeekItem {
    /// The elapsed clock.
    Elapsed,
    /// The track line itself, click or drag to seek.
    Strip,
    /// The ending clock: time left, or the full duration when toggled.
    Ending,
    /// The full length, always: pairs with the elapsed clock for the
    /// classic "elapsed, total" read without giving up the countdown.
    Duration,
    /// A flexible gap that pushes the pieces around it apart; a row
    /// holds as many as the layout needs.
    Spacer,
    /// A spacer that draws a hairline in the border color across its gap.
    Divider,
    /// The line break: everything after it drops to a second row. The
    /// stacked layouts, where the strip runs the full width with its
    /// clocks over or under it instead of beside.
    Break,
}

/// The two clocks, what the quick Show Timings toggle moves as a pair.
const CLOCKS: [SeekItem; 2] = [SeekItem::Elapsed, SeekItem::Ending];

/// The row's full catalog in stock order: what the arrange editor offers,
/// and where a menu toggle slots a re-shown piece back in.
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

/// What the strip does with the part of the live buffer that hasn't taped
/// yet, the stretch left of the tape on a station's strip.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LeadIn {
    /// A broken line: buffer that exists as a setting and not yet as
    /// bytes. The default, since it says outright that the bar isn't
    /// growing, it's filling.
    #[default]
    Dashed,
    /// A solid line, fainter than the tape's wash. The same statement
    /// without the texture, for a thin strip where dashes read as noise.
    Faint,
    /// Nothing at all, so the bar is only ever as long as the tape. Back
    /// to a strip that grows, for anyone who preferred it.
    Hidden,
}

/// The seek panel's per-view config: what a saved layout restores, and
/// what the panel's dropdown menu edits. Deserialization routes through
/// [`SeekConfigDump`] so layouts from before the row became an ordered
/// list still read.
#[derive(Clone, Serialize, Deserialize)]
#[serde(from = "SeekConfigDump")]
pub struct SeekConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    /// The ending clock shows the full duration instead of the time left;
    /// the panel settings' Ending row flips it.
    pub show_total: bool,
    /// A thin line at the scrobble threshold, where the playing track
    /// counts as listened for Last.fm. Only draws while scrobbling is
    /// connected and on.
    pub scrobble_marker: bool,
    /// The playing track's bookmarks as chevrons under the line, each one
    /// a seek and a right-click menu.
    pub bookmarks: bool,
    /// The track line's height in px.
    pub thickness: f32,
    /// The track line's corner radius in px, capped at a pill.
    pub rounding: f32,
    /// The playhead's width in px.
    pub playhead_width: f32,
    /// The playhead spans the strip's full height; off, it hugs the line.
    pub playhead_full: bool,
    /// Cap the full playhead's height in px, kept centered on the line;
    /// 0 lets it fill the panel.
    pub playhead_max: f32,
    /// What the unfilled part of a station's buffer looks like.
    pub live_lead_in: LeadIn,
    /// The lead-in dash's length in px, the gap after it to match.
    pub live_dash_len: f32,
    /// A band of extra weight travels along the lead-in, the way a loading
    /// bar's does. Off by stock: a strip that moves on its own is a thing
    /// to ask for, not a thing to find running.
    pub live_sweep: bool,
    /// The shown pieces in display order; one not listed is hidden.
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

/// The dump shape [`SeekConfig`] deserializes through: the ordered list
/// newer layouts write, or the retired `timings` toggle that was both
/// clocks around the strip.
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

/// The lead-in dash's stock length in px, and the band the slider picks
/// across. Four is a dash at the strip's usual thickness rather than a row
/// of pills; past the top the line reads as a measure rather than a
/// texture. The floor is a single pixel, where the dashes stop reading as
/// dashes and turn into a comb.
const DASH_LEN_DEFAULT: f32 = 4.0;
const DASH_LEN_MIN: f32 = 1.0;
const DASH_LEN_MAX: f32 = 24.0;

fn default_dash_len() -> f32 {
    DASH_LEN_DEFAULT
}

impl From<SeekConfigDump> for SeekConfig {
    fn from(dump: SeekConfigDump) -> Self {
        let items = match dump.items {
            // Deduped row by row, the breaks put back after: the catalog
            // doesn't include the break (it draws as the editor's row
            // boundary, not a chip), and each row may hold its own copy
            // of a piece.
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

/// The seek strip: the waveform minus the peaks, a track line with the
/// played side in the accent and a playhead, click or drag to seek, the
/// elapsed and remaining clocks at its ends. Position and seek come off
/// the player the same way the waveform's do.
pub struct SeekStripPanel {
    state: AppState,
    config: SeekConfig,
    /// The strip's painted bounds and drag state, for scrub mapping.
    scrub: ScrubState,
    /// The settings page's scalar strips, with the shared readout edit.
    thickness_scrub: ScrubState,
    rounding_scrub: ScrubState,
    playhead_scrub: ScrubState,
    playhead_max_scrub: ScrubState,
    value_edit: ValueEdit,
    focus: FocusHandle,
    /// The tab panel that currently hosts this panel, for duplicate and pop-out.
    tab_panel: Option<WeakEntity<TabPanel>>,
    /// The row as it stood when the quick Show Timings toggle last hid the
    /// clocks, so turning them back on returns them to where they were
    /// rather than their catalog rank. Held on the panel and not the config
    /// because it's the undo for one toggle, not a layout anybody saves.
    timings_stash: Option<Vec<SeekItem>>,
    /// The playing track's bookmarks and which track they were read for,
    /// re-read on a track change and on a bookmark edit rather than on
    /// every pump tick the strip repaints on.
    marks: Vec<Bookmark>,
    marks_key: Option<TrackKey>,
    /// The bookmark ribbon the pointer is on, for its readout.
    hover_mark: Option<i64>,
    /// The playing track's session cues, cached on the same terms as the
    /// bookmarks above and for the same reason.
    cues: Vec<Cue>,
    cues_key: Option<TrackKey>,
    /// The cue chevron the pointer is on, for its readout.
    hovered_cue: Option<u64>,
    /// The station song chevron the pointer is on, for its readout. Its
    /// own field rather than the cue's: a station has no cues, but the two
    /// hover states outlive one render apiece and mixing them would leave
    /// a stale id behind on the switch from a file to a stream.
    hovered_song: Option<u64>,
    /// Where the pointer last was along the strip, 0 to 1. Only the song
    /// chevrons write it, because they're the only marks that move.
    pointer_at: Option<f32>,
    /// The dash length slider's scrub, the settings page's Live section.
    dash_scrub: ScrubState,
    /// Where the last right click on bare strip landed, as a position in
    /// the track. The insert menu builds a frame after the press and never
    /// sees the event, so the position is parked here on the way past.
    insert_at_ms: Arc<AtomicU32>,
    /// Time zero for the LIVE mark's pulse. The strip has no clock of its
    /// own otherwise: it redraws off the pump, and a stream that hasn't
    /// opened yet isn't moving the pump.
    epoch: Instant,
    _player_changed: Subscription,
    _library_changed: Subscription,
    _cues_changed: Subscription,
}

impl SeekStripPanel {
    pub fn new(state: AppState, config: SeekConfig, cx: &mut Context<Self>) -> Self {
        // The clock and the playhead move every tick, so this one uses the
        // raw per-pump notify, not the gated observe the other panels use.
        let _player_changed = cx.observe(&state.player, |_, _, cx| cx.notify());
        // A bookmark edit anywhere (the M key, the panel, this strip's own
        // menu) drops the cached marks so the next paint reads them again.
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
        // A cue dropped or taken off the track this strip is drawing. The
        // event names its track, so a strip on a different song ignores it
        // rather than throwing away a set that didn't move.
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

    /// The playing track's bookmarks, read once per track (and again after
    /// an edit), so the per-tick repaint never touches the database.
    fn marks_for(&mut self, key: &TrackKey, cx: &App) -> &[Bookmark] {
        if self.marks_key.as_ref() != Some(key) {
            self.marks = self.state.library.read(cx).bookmarks_for(key);
            self.marks_key = Some(key.clone());
            self.hover_mark = None;
        }
        &self.marks
    }

    /// The playing track's session cues, on the same terms as the
    /// bookmarks above: read once per track and again after an edit.
    fn cues_for(&mut self, key: &TrackKey, cx: &App) -> &[Cue] {
        if self.cues_key.as_ref() != Some(key) {
            self.cues = self.state.cues.read(cx).for_key(key);
            self.cues_key = Some(key.clone());
            self.hovered_cue = None;
        }

        &self.cues
    }

    /// Apply a fraction along the strip. On a track that's a position, on a
    /// station it's a point in the timeshift tape, measured back from the
    /// live edge at the right end. Read fresh off the player rather than
    /// off the frame that armed the handler: a drag outlives the paint that
    /// started it, and the entry underneath can change mid-drag.
    fn seek_at(&self, fraction: f32, cx: &App) {
        let player = self.state.player.read(cx);
        match player.now_playing().and_then(|now| now.shift) {
            Some(shift) => player.seek_live(shift_behind(fraction, &shift)),

            None => panel::seek_fraction(&self.state.player, fraction, cx),
        }
    }

    /// Drop a song hover the pointer isn't on any more.
    ///
    /// Every other mark in this panel is nailed to a position in a track,
    /// so it only leaves the pointer when the pointer leaves it and the
    /// hover-out clears the readout. A station's marks walk left as the
    /// broadcast rolls on, under a pointer that never moved, and nothing
    /// fires when they do: the readout would sit there naming a song that
    /// has since slid out from under the cursor. So the hover is checked
    /// against where the pointer actually is on every paint instead of
    /// waiting for an event that isn't coming.
    fn settle_song_hover(&mut self, marks: &[cue_ui::CueMark]) {
        let Some(id) = self.hovered_song else {
            return;
        };

        // In fractions rather than pixels, since that's what both the
        // pointer and the mark are kept in; the slot's half width is the
        // only thing that has to cross over.
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

    /// Whether either clock is on the row, what the quick timings toggle
    /// reads and flips.
    fn timings_shown(&self) -> bool {
        self.config
            .items
            .iter()
            .any(|i| matches!(i, SeekItem::Elapsed | SeekItem::Ending))
    }

    /// Both clocks on or off in one move, the row they were in kept across
    /// the round trip.
    fn toggle_timings(&mut self) {
        self.config.items =
            panel::toggled_stashed(ITEMS, &self.config.items, &mut self.timings_stash, &CLOCKS);
    }

    /// The panel's own dropdown entries: the quick timings and marker
    /// toggles. Timings still means both clocks at once; the settings
    /// window's arrange editor splits and reorders them.
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

        // The station rows sit in their own section: they draw nothing at
        // all off a stream, and mixing them into the list above would have
        // most of a page that only applies some of the time.
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

/// The strip's paint knobs, copied off the config for the paint closure.
#[derive(Clone, Copy)]
struct StripLook {
    thickness: f32,
    rounding: f32,
    playhead_width: f32,
    playhead_full: bool,
    playhead_max: f32,
    /// What the unfilled buffer looks like on a station's strip, and the
    /// dash it's drawn with when that's dashes.
    lead_in: LeadIn,
    dash_len: f32,
    /// Where the lead-in's sweep has got to, or None with the sweep off.
    /// The config says whether the band runs, the panel's own clock says
    /// where it is, so this is set at the call rather than copied.
    sweep: Option<Sweep>,
    /// Halve every alpha: what a paused station's line looks like. The one
    /// knob here that isn't the config's. It rides along anyway, because on
    /// screen it's part of how the line looks, and the paint already takes
    /// this bag rather than a knob per argument.
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

/// The track line centered in whatever height the panel gets: unplayed side
/// dim, played side solid, the waveform's playhead on top. `look` holds
/// the config's line and playhead knobs, the radius capped at a pill.
/// `marker` draws the scrobble threshold as a thin full-height line under
/// the playhead, `ab` the repeat section's ends and wash under that,
/// `marks` the bookmark ribbons along the bottom edge and `cues` the
/// session chevrons along the top.
///
/// A dimmed look halves every alpha, the same statement the live marks make
/// for a paused station: nothing is on air, and the strip shouldn't sit
/// there at full strength saying otherwise. Everything stays where it is
/// rather than dropping out, since the tape behind the playhead is still
/// there to be scrubbed through while the station is hung up.
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

/// The playhead `head_x` px into the strip: the panel's full height, capped
/// when the config says so, or the line's when it hugs. Either way it
/// centers on the line. Its own function because the station's strip draws
/// the same head over a different bar.
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

/// The config's list cut at the break into one piece list per row. No
/// break reads as the single row the panel has always drawn, and a break
/// with nothing on a side drops the empty row rather than rendering it.
fn split_rows(items: &[SeekItem]) -> Vec<Vec<SeekItem>> {
    items
        .split(|i| matches!(i, SeekItem::Break))
        .filter(|row| !row.is_empty())
        .map(|row| row.to_vec())
        .collect()
}

/// [`split_rows`] for the rows editor, empty rows kept: an added row's
/// well shows until a piece lands or its x drops it.
fn editor_rows(items: &[SeekItem]) -> Vec<Vec<SeekItem>> {
    items
        .split(|i| matches!(i, SeekItem::Break))
        .map(|row| row.to_vec())
        .collect()
}

/// Tabular digits for the clock, built once: [`clock`] runs twice per
/// pump tick while playing, so the feature list shouldn't reallocate
/// every call.
static TNUM: LazyLock<FontFeatures> =
    LazyLock::new(|| FontFeatures(Arc::new(vec![("tnum".into(), 1)])));

/// A clock beside the strip: muted, fixed in the row, digits tabular so a
/// tick never changes the text width.
fn clock(text: String) -> Div {
    let mut clock = div().flex_none().text_color(palette::text_muted());
    clock
        .text_style()
        .get_or_insert_with(Default::default)
        .font_features = Some(TNUM.clone());
    clock.child(text)
}

/// How long one breath of the opening pulse takes, seconds. Slow enough to
/// read as waiting rather than as something wrong; a fast blink is an
/// alarm, and a stream taking a second to open is not one.
const PULSE_SECS: f32 = 1.6;

/// The pulse's weight at `t` seconds in: a sine eased between a dim floor
/// and full, so the mark breathes instead of flashing.
fn pulse(t: f32) -> f32 {
    let phase = t / PULSE_SECS * std::f32::consts::TAU;
    0.45 + 0.55 * (phase.sin() * 0.5 + 0.5)
}

/// How long the lead-in's sweep takes to cross it once, seconds. Slow
/// enough to read as one band walking across a bar that's filling, rather
/// than as a flicker over it.
const SWEEP_SECS: f32 = 2.2;

/// How far the band reaches either side of its crest, as a share of the
/// lead-in's length.
const SWEEP_BAND: f32 = 0.25;

/// What the crest adds on top of the lead-in's resting weight, as a
/// multiple of it.
const SWEEP_GAIN: f32 = 1.6;

/// The longest stretch of lead-in drawn as one quad while the sweep runs,
/// px. A quad shades between its two ends and nowhere else, so a wider one
/// would flatten the crest sitting inside it.
const SWEEP_STEP: f32 = 8.0;

/// Where the sweep's crest sits along the lead-in right now: 0 at the left
/// end of the unfilled stretch, 1 where the tape starts.
#[derive(Clone, Copy)]
struct Sweep(f32);

impl Sweep {
    /// The crest's place at `phase` seconds in. It sets off a band's reach
    /// short of the left end and finishes the same distance past the right,
    /// so the band walks on and off instead of appearing mid-line.
    fn at(phase: f32) -> Self {
        let trip = (phase / SWEEP_SECS).rem_euclid(1.0);
        Sweep(-SWEEP_BAND + trip * (1.0 + 2.0 * SWEEP_BAND))
    }

    /// The alpha the lead-in draws at `along` it, 0 to 1: its resting
    /// weight plus the band's, wherever the band reaches.
    fn alpha(self, base: u8, along: f32) -> u8 {
        let reach = ((along - self.0).abs() / SWEEP_BAND).min(1.0);
        // A cosine rather than a straight ramp, so the band's edges taper
        // off instead of ending on a line you can see.
        let weight = 0.5 * (1.0 + (reach * std::f32::consts::PI).cos());
        (f32::from(base) * (1.0 + SWEEP_GAIN * weight)).min(255.0) as u8
    }
}

/// How dim the live marks go while the station is paused. Steady rather
/// than breathing: nothing is being waited for, the stream is simply off.
const PAUSED_OPACITY: f32 = 0.4;

/// How a live stream's mark looks in the state it's in: the color, and the
/// opacity the pulse has it at. Shared with the waveform panel's corner
/// mark, so a stream that's reconnecting reads the same way on both.
///
/// Paused wins over the stream state. A pause hangs the station up, so
/// there's no audio arriving and nothing to wait for, and a mark still in
/// the accent would say "on air" about a silent strip.
pub(crate) fn live_tint(stream: Option<StreamState>, paused: bool, t: f32) -> (gpui::Rgba, f32) {
    if paused {
        return (palette::text_muted(), PAUSED_OPACITY);
    }

    match stream {
        Some(StreamState::Opening) | Some(StreamState::Reconnecting) => {
            (palette::text_muted(), pulse(t))
        }

        Some(StreamState::Dropped) => (palette::tone_bad(), 1.0),

        // Live, and the moment before the first report lands: the mark the
        // strip has always drawn.
        Some(StreamState::Live) | None => (palette::accent(), 1.0),
    }
}

/// Whether the playhead is standing on the live edge. An exact compare,
/// because the engine snaps the distance to zero at the edge: rounding a
/// near-zero off on this side too would put a second boundary next to that
/// one, and the clock would flicker across it.
fn at_live_edge(behind_secs: f64) -> bool {
    behind_secs <= 0.0
}

/// Whether the station is playing out of its tape rather than off the edge.
/// Shared with the waveform's corner mark, so both say the same thing about
/// where the playhead is.
pub(crate) fn behind_live(shift: Option<&Shift>) -> bool {
    shift.is_some_and(|shift| !at_live_edge(shift.behind_secs))
}

/// Where the playhead sits on a timeshift strip. The strip spans the whole
/// buffer the setting allows, not the seconds taped so far, so the bar
/// keeps its meaning while the tape fills instead of stretching under the
/// playhead for the first ten minutes of a station.
fn shift_progress(shift: &Shift) -> f32 {
    if shift.cap_secs <= 0.0 {
        return 1.0;
    }

    // Held to the tape: the head belongs on the recorded side of the line,
    // and a report a hair past the oldest byte would put it in the lead-in
    // where nothing can be played from.
    let behind = shift.behind_secs.clamp(0.0, shift.window_secs);

    (1.0 - behind / shift.cap_secs).clamp(0.0, 1.0) as f32
}

/// How much of the strip is tape: the seconds held against the cap, sitting
/// against the right end. What's left of it is buffer the connection hasn't
/// filled yet, which the strip draws as a dashed lead-in.
fn shift_held(shift: &Shift) -> f32 {
    if shift.cap_secs <= 0.0 {
        return 1.0;
    }

    (shift.window_secs / shift.cap_secs).clamp(0.0, 1.0) as f32
}

/// The mapping backwards: a fraction along the strip as how far behind the
/// live edge the point under it is, which is what a click or a drag seeks
/// to. Held to what the tape has, so a grab at the dashed lead-in lands on
/// the oldest thing there is to play rather than in a silence that was
/// never recorded.
fn shift_behind(fraction: f32, shift: &Shift) -> f64 {
    let behind = (1.0 - fraction.clamp(0.0, 1.0) as f64) * shift.cap_secs.max(0.0);

    behind.min(shift.window_secs.max(0.0))
}

/// Where a right click on the strip points, in milliseconds into the
/// track: the fraction under the pointer against the length. Its own
/// function because it's the whole contract between a press and the mark
/// the insert menu drops, and the only part of that worth a test.
pub(crate) fn insert_position_ms(fraction: f32, duration_secs: f64) -> u32 {
    let secs = fraction.clamp(0.0, 1.0) as f64 * duration_secs.max(0.0);

    // A track long enough to overflow this is 49 days, so the clamp is
    // for a garbage duration rather than for real music.
    (secs * 1000.0).round().clamp(0.0, u32::MAX as f64) as u32
}

/// The strip's own right click: an inert layer over the whole strip that
/// parks the position under the pointer and opens the insert menu there.
/// Both strips host it, which is why it lives here rather than in either
/// of their bodies.
///
/// Three orderings hold this together, all of them registration order
/// during paint. The menu builds a frame after the press and never sees
/// the event, so the press stores the position for it. The press then
/// stops, because the dock's body handler would otherwise open the panel
/// dropdown stacked over the menu. And the layer goes in before the mark
/// overlays, so a right click on a chevron reaches that mark's own menu
/// and this one never runs.
///
/// Stopping the press is also why the panel's own dropdown gets appended
/// here: the body handler that would have opened it never runs, and a
/// right click on the strip that offered nothing but the two mark rows
/// would put the panel's settings out of reach over most of the panel.
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

/// The station's song changes placed along its strip: one chevron per
/// title the tape is publishing, mapped back from the live edge against
/// the whole buffer the way the playhead is.
///
/// Everything that arrives is drawn. The tape clips the list to what it
/// still holds and measures it against the playhead's own edge, so a mark
/// whose audio has been trimmed off the back never reaches here, and a second
/// opinion on that in the panel would only be a second place for the two
/// to disagree. The id is the index, which is all the hover layer needs to
/// tell one mark from another.
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
            // The chevron carries no time of its own: the readout over it
            // is the song's name, and the click seeks by the distance the
            // mark itself holds.
            position_ms: 0,
        })
        .collect()
}

/// The station's reconnects placed along its strip, the same mapping back
/// from the live edge the songs take.
///
/// A fraction and nothing else. There's no hover on these and nothing to
/// click: a break is the one place on the strip a seek won't go, so the
/// only thing it has to do is be visible before somebody aims past it.
fn tape_gaps(gaps: &[LiveGap], shift: &Shift) -> Vec<f32> {
    if shift.cap_secs <= 0.0 {
        return Vec::new();
    }

    gaps.iter()
        .map(|gap| (1.0 - gap.behind_secs / shift.cap_secs).clamp(0.0, 1.0) as f32)
        .collect()
}

/// What the ending slot holds while a station plays: the distance back to
/// the live edge once the playhead has left it. None at the edge, where the
/// LIVE mark stands instead.
///
/// Whole seconds, floored. The distance slides with the drawn edge, so a
/// floor ticks once a second the way a countdown does, and rounding would
/// only move the tick half a second earlier.
fn behind_clock(shift: Option<&Shift>, digits: usize) -> Option<String> {
    let shift = shift.filter(|shift| !at_live_edge(shift.behind_secs))?;

    Some(format!(
        "-{}",
        fmt_time_padded(shift.behind_secs.floor(), digits)
    ))
}

/// The LIVE mark's face: [`live_tint`] plus where the playhead is. Back in
/// the tape the mark stops reporting the stream and starts offering the way
/// forward, so it gives up the accent for a plain muted face. What the
/// stream itself is doing still wins, since a reconnect or a drop is the
/// bigger news either way.
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

/// What stands where the ending clock stands while a station plays. The
/// clock's own shape so the row's widths don't jump between a file and a
/// stream, and its color says where the stream stands: the accent while
/// audio is arriving, muted and breathing while it's being waited for, the
/// bad tone once the reconnects have run out. The three states that aren't
/// plain playback carry a tooltip, since a color is not a sentence.
///
/// It's also the button back to the live edge, wherever it's drawn. At the
/// edge the click does nothing, and the mark is a button there anyway: one
/// that appears the moment it would work is one nobody knows is there. Two
/// slots can draw a mark at once, so each passes its own element id.
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

/// The ending slot once the playhead has left the live edge: how far back
/// it sits, in the clock's own shape, and the button back to the edge.
///
/// The clock takes the mark's job here because it has taken the mark's
/// place. On the stock row this slot is the only live control on screen,
/// and a distance from the edge with no way to close it is a readout of a
/// problem. So the minus sign says where you are and the click undoes it.
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

/// The song on air as one line. A station that sends one unsplittable
/// field leaves the artist empty, and the title alone is then the whole of
/// what it said. The stations panel spells this the same way; it's five
/// lines and neither module owns the other.
fn song_text(artist: &str, title: &str) -> String {
    if artist.is_empty() {
        return title.to_string();
    }

    format!("{artist} - {title}")
}

/// The interactive layer over a station's song marks: a hit target per
/// chevron that reports its hover and, on a click, plays the tape from
/// that song's first second. [`cue_ui::overlay`]'s shape, with the song's
/// name in the readout instead of a time and none of the editing, because
/// these marks are the station's and there's nothing here to remove.
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
            // The strip's own preview would keep tracking the pointer under
            // the chevron; clearing it here leaves the song's readout as
            // the only one showing. The pointer's place is kept anyway,
            // because the tape walks marks out from under a still pointer
            // and [`SeekStripPanel::settle_song_hover`] needs somewhere to
            // check it against.
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
                hover_scrub.set_hover(None);
                this.pointer_at = hover_scrub.fraction(event.position.x);
                cx.stop_propagation();
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.hovered_song = hovered.then_some(id);
                cx.notify();
            }))
            // The click lands on the song's own mark rather than the pixel
            // under the pointer, and the strip's seek stays out of it.
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

/// The hit target around a song chevron, wider than the drawing so a
/// pointer finds it without aiming. The cue layer's width, since the two
/// draw the same chevron.
const SONG_HIT_W: f32 = 16.0;

/// The hovered song's readout: its name in the seek preview's pill, under
/// the chevron rather than over it, since the chevron is already sitting on
/// the top edge.
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

/// The seek preview over a station's strip: how far behind the live edge
/// the point under the pointer is, held to what the tape has, so the pill
/// over the dashed lead-in reads the oldest second and stops moving.
/// [`panel::seek_hover`]'s shape and behavior with the tape's clock in it,
/// since a point on a broadcast has no absolute time to name it by.
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
        // The pointer leaving the strip stops the moves, so the leave has
        // to clear the readout itself.
        .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
            if !hovered && left.set_hover(None) {
                cx.notify();
            }
        }))
        .when_some(hover, |d, fraction| {
            d.child(shift_pill(fraction, shift_behind(fraction, &shift)))
        })
}

/// The timeshift preview label: how far back the point at `fraction` sits,
/// a pill centered over it near the top of the strip. The right end says
/// LIVE rather than -0:00, which is where a click there lands.
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
                // The zero-width column above gives the text no room, so
                // the time would wrap to one glyph per line without this.
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

/// The lead-in's line: where it sits on screen, and how long the whole
/// unfilled stretch is. The length is what the sweep is measured against,
/// so every dash in the run shades off the same band rather than each one
/// carrying a band of its own.
#[derive(Clone, Copy)]
struct LeadLine {
    /// The strip's left edge and the line's top, in window space.
    left: Pixels,
    top: Pixels,
    /// The line's height and corner radius, px.
    height: f32,
    radius: f32,
    /// How far the unfilled stretch runs, px.
    span: f32,
    sweep: Option<Sweep>,
}

impl LeadLine {
    /// Paint one stretch of it: a dash, or the whole lead-in when the line
    /// is the solid kind. `x` and `width` are px in from the strip's left
    /// edge, `alpha` the weight it rests at with no band on it.
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

        // A quad shades between its own two ends and nowhere in between, so
        // a wide stretch is cut up first: the crest has to be able to sit
        // inside the line rather than only at one end of it. The cuts keep
        // the radius on the stretch's outer corners and stay square at the
        // seams, so a rounded dash still reads as one dash.
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

/// How wide the break a reconnect cuts in the bar is, px: the hole taken
/// out of it, and the line standing in the middle of the hole. Narrow on
/// purpose. It marks a splice the listener can't cross, not a stretch of
/// missing time, and a wide block would read as a length of silence that
/// the tape is holding.
const GAP_BREAK_W: f32 = 5.0;
const GAP_LINE_W: f32 = 1.5;
/// The notch over the break: how wide it is and how far above the bar it
/// stands. Enough to catch the eye on a bar three pixels thick, where the
/// break alone is a missing pixel or two. The height is public to the
/// crate because the waveform has to leave the notch room at the top of
/// its own strip before it hands the band over.
const GAP_NOTCH_W: f32 = 5.0;
pub(crate) const GAP_NOTCH_H: f32 = 3.0;
/// The break's alpha at full weight.
const GAP_ALPHA: u8 = 0xcc;

/// Draw the tape's reconnects over a strip: each one a break cut out of
/// the bar, a thin line standing in the break, and a notch across the top
/// of it.
///
/// Its own shape in its own place, which is the rule the other marks on
/// these strips already keep. Session cues hang off the top edge as
/// chevrons and bookmarks off the bottom as ribbons, so a gap goes through
/// the middle and touches neither edge, and it stays in the muted tone
/// rather than the accent: nobody put it there, it's the broadcast
/// missing.
///
/// `band` is the top and the height of the cut in strip-local px, the line
/// itself on the seek strip and most of the panel on the waveform.
/// `weight` scales the alpha the way the mark painters' does, for a strip
/// that has dimmed.
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
    // The notch takes whatever room there is above the cut, so a strip too
    // short for the full height gets a shorter one instead of losing it.
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

        // The hole first, in the panel's own background: the break has to
        // read as the bar stopping rather than as something drawn on top
        // of a bar that carries on underneath.
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

/// A station's strip: the whole buffer the setting allows, with the tape
/// held against its right end, where the live edge is.
///
/// The dashes on the left are buffer that exists as a number in settings
/// and not yet as bytes. A fresh station is all dashes and fills in
/// leftwards as it tapes, so the bar reads the same from the first second
/// instead of growing out of nothing under the playhead. Nothing there is
/// reachable: a click in it lands on the oldest thing held, and the head
/// never crosses into it.
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
    // Where the tape starts, and where in it the playhead is.
    let tape_x = (1.0 - shift_held(shift)) * w;
    let head_x = (shift_progress(shift) * w).max(tape_x);

    // The lead-in: dashes, a fainter solid line, or nothing, whichever the
    // config asks for. The dash's gap matches its length, so one number
    // sets the texture.
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

        // A dash the width of the whole lead-in is a solid line, at half
        // the tape's weight so the two still read apart.
        LeadIn::Faint => lead(tape_x, if look.dim { 0x0d } else { 0x1a }, window),

        LeadIn::Hidden => {}
    }

    // The tape: the wash across everything held, the accent over the part
    // of it that has been heard.
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

    // Where the connection broke: the bar stops and starts again. It goes
    // on after both fills, since a break in the tape is a break in
    // whichever of them covers it, and before the playhead, which crosses
    // over it the way it crosses everything else.
    paint_gaps(
        gaps,
        (f32::from(line_y - bounds.origin.y), line_h),
        if look.dim { 0.5 } else { 1.0 },
        bounds,
        window,
    );

    // The station's own marks, where its songs turned over. Chevrons off
    // the top edge, the same ones a track's cues get: a point in what's
    // playing that somebody else put there.
    cue_ui::paint_marks(songs, 1.0, bounds, window);
    paint_playhead(head_x, look, bounds, window);
}

/// The strip while a station plays with no tape behind it yet: the line at
/// full width and nothing moving along it. Same thickness and rounding the
/// config gives the real strip, so the row keeps its shape; what goes is
/// the played side, the playhead, and every mark that needs a fraction to
/// sit at. The first seconds of a stream draw this, and it's what a station
/// looked like before the buffer existed.
fn paint_live_strip(look: StripLook, bounds: Bounds<Pixels>, window: &mut Window) {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w <= 0.0 || h <= 0.0 {
        return;
    }

    // The bar dims with the marks: a paused station is a station with
    // nothing on air, and the line says so at the same strength.
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
        // The panel is a focus stop: a click puts the keyboard here and
        // tab walks to it, which is also what puts its tab group on the
        // focus path for the tab-cycle chord.
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(window, cx).track_focus(&focus))
    }
}

impl SeekStripPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let player = self.state.player.read(cx);
        let now = player.now_playing();
        let ab = player.ab_state();

        // No frame polling: the raw observe in `new` re-renders the strip
        // on every pump tick while audio moves, which is the rate the clock
        // and playhead actually change at. A per-frame request on top only
        // redraws identical pixels. It also kept the whole window repainting
        // at refresh rate through a paused session. Scrub drags notify on
        // their own through the mouse handlers.

        let root = div()
            .size_full()
            .bg(palette::bg_root())
            .flex()
            .flex_col()
            .justify_center();

        let Some(now) = now else {
            // Idle: the strip stays blank until a session brings a track.
            return root;
        };

        // A station has no timeline of its own. There is no fraction for a
        // mark to sit at and no end for a countdown to count towards, so
        // everything that draws against a track's length drops out rather
        // than drawing against a zero.
        let live = now.live;
        // The timeshift tape the engine holds behind the stream: how long
        // it is, and how far back in it the playhead has fallen. None off a
        // station, and for the first seconds of one, before the tape has a
        // byte to scrub through.
        let shift = now.shift;
        // Where the stream stands, and the pulse phase the mark breathes
        // on. A stream waiting to open moves nothing else on the strip, so
        // this is the one thing here that asks for frames of its own.
        let stream = now.stream;
        // A paused station is hung up, so the marks dim to a steady muted
        // face and nothing breathes: there's nothing being waited for.
        let paused = live && !player.is_playing();
        let waiting = live
            && !paused
            && matches!(
                stream,
                Some(StreamState::Opening) | Some(StreamState::Reconnecting)
            );
        // The lead-in's band moves on the clock rather than on the pump, so
        // it asks for frames the same way the opening pulse does. A paused
        // station gets none: nothing is filling, so nothing is loading. Nor
        // does a tape that has filled the bar, or one that hasn't started,
        // since neither draws a lead-in for the band to walk along, and a
        // station left playing overnight shouldn't hold the window at
        // refresh rate for a stretch of strip that isn't there.
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
        // The marker only shows where a scrobble could actually happen: the
        // toggle on and some destination armed.
        let marker = (!live && self.config.scrobble_marker)
            .then(|| self.state.scrobble_marker(cx))
            .flatten();
        // The A-B section, or the lone A while the cycle waits for B.
        let ab = (!live)
            .then(|| panel::ab_fractions(ab, now.duration_secs))
            .flatten();
        // Everything keyed to a position in the track drops out together
        // while a station plays: the marks, the layer that drops new ones,
        // and the menus over both.
        let positional = !live && position_bound::allowed(&self.state, cx);
        // The track's bookmarks, placed along the strip.
        let marks = if self.config.bookmarks && positional {
            bookmark_ui::marks(self.marks_for(&now.key, cx), now.duration_secs)
        } else {
            Vec::new()
        };
        let hover_mark = self.hover_mark;
        // The track's session cues, on the top edge opposite them.
        let cues = if positional {
            cue_ui::marks(self.cues_for(&now.key, cx), now.duration_secs)
        } else {
            Vec::new()
        };
        let hovered_cue = self.hovered_cue;
        // The station's song changes, off the tape rather than the library.
        // Read every paint and not cached on the title revision: each mark's
        // distance from the live edge grows with the broadcast, so a set
        // held from the last title would slide out of place under the bar.
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
        // The station's reconnects, read and mapped on the same terms as
        // the songs: the tape publishes both against one edge, and the two
        // would drift apart if the strip took them a tick apart.
        let gap_marks = shift
            .as_ref()
            .map(|shift| tape_gaps(&self.state.player.read(cx).live_gaps(), shift))
            .unwrap_or_default();
        // The seek click is on the track alone so the clocks beside it
        // stay inert.
        // The seek preview shows once the duration resolves; before that a
        // fraction maps to nothing.
        let hover_duration = now.duration_secs.filter(|d| *d > 0.0 && !live);
        // A file always maps, and a station maps once it has a tape: the
        // strip is the buffer then, and a point on it is a point in what's
        // been held.
        let seekable = !live || shift.is_some();
        // The insert layer needs a length to map a press onto, so it waits
        // for the duration the same way the seek preview does.
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
                            // A station with nothing held yet: the flat bar,
                            // and no drag to arm over it.
                            if live && shift.is_none() {
                                paint_live_strip(look, bounds, window);
                                return;
                            }

                            match &shift {
                                // The buffer: dashed where it hasn't filled,
                                // heard behind the playhead, held and unheard
                                // ahead of it, live at the right end. No
                                // marks, since none of them have a position
                                // on a broadcast to sit at.
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
            // The station's own preview, reading back from the live edge
            // rather than forward from a track's start.
            .when_some(shift, |d, shift| {
                d.child(shift_hover(&self.scrub, shift, cx))
            })
            // The marks' hit layers go over the seek readout's, so a
            // pointer on a mark reads the mark.
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
            // The station's songs, on the same edge the cues use, which a
            // station never has any of.
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

        // The clocks around the strip: the ending one counts down, or
        // shows the full duration when toggled, and "-:--" until the
        // duration resolves. Minutes pad to the duration's digits so
        // neither clock changes width mid-track and wiggles the strip.
        let digits = now
            .duration_secs
            .map(|d| (d as u64 / 60).to_string().len())
            .unwrap_or(1);
        // A station's position counts the whole listen, and the clock over
        // it shows the song: it goes back to zero when the stream says the
        // next one started. Off a station there is no song start and this is
        // the position it always was.
        let elapsed = song_clock(now.position_secs, now.song_start_secs);
        let ending = match now.duration_secs {
            Some(d) if self.config.show_total => fmt_time_padded(d, digits),
            Some(d) => format!(
                "-{}",
                fmt_time_padded((d - now.position_secs).max(0.0), digits)
            ),
            None => "-:--".into(),
        };
        // A station's ending slot counts back to the live edge the way a
        // track's counts down to its end. None at the edge, where the LIVE
        // mark has the slot instead.
        let behind = behind_clock(shift.as_ref(), digits);

        // The config's list draws in order, cut into rows at the break:
        // each shown piece in its place, whatever order the arrange
        // editor left them in. The strip's row takes whatever height the
        // others leave, so a stacked layout keeps the strip broad and its
        // clocks in a thin line over or under it.
        let mut track = Some(track);
        let mut piece = |item: &SeekItem| -> Option<AnyElement> {
            match item {
                SeekItem::Elapsed => {
                    Some(clock(fmt_time_padded(elapsed, digits)).into_any_element())
                }
                SeekItem::Strip => track.take().map(|t| t.into_any_element()),
                // A station's ending slot says LIVE while the playhead is
                // at the edge, since a countdown to an end that never comes
                // is worse than no clock at all, and counts back to the
                // edge once the playhead has left it. Either face is the
                // button back to live.
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
                // The duration slot keeps the mark whatever the playhead is
                // doing, so a row showing both has the way back to the edge
                // beside the distance from it.
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
                // Any clock brings its row's padding in; a row of the
                // strip alone (spacers included) runs edge to edge.
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

    /// Where a right click drops a mark: the fraction under the pointer
    /// read against the track's length, with the ends held on the strip.
    #[test]
    fn a_right_click_lands_at_its_fraction_of_the_track() {
        assert_eq!(insert_position_ms(0.0, 120.0), 0);
        assert_eq!(insert_position_ms(0.5, 120.0), 60_000);
        assert_eq!(insert_position_ms(1.0, 120.0), 120_000);

        // Sub-millisecond aim rounds rather than truncating, so the mark
        // sits where the pointer was and not a hair before it.
        assert_eq!(insert_position_ms(0.5, 0.001), 1);

        // A drag that overshot the ends, and a length that never resolved:
        // both land on the strip rather than off it.
        assert_eq!(insert_position_ms(-0.5, 120.0), 0);
        assert_eq!(insert_position_ms(1.5, 120.0), 120_000);
        assert_eq!(insert_position_ms(0.5, 0.0), 0);
        assert_eq!(insert_position_ms(0.5, -30.0), 0);

        // A garbage duration can't wrap the position round to a small one.
        assert_eq!(insert_position_ms(1.0, 1.0e12), u32::MAX);
    }

    /// The playhead rides the right end at the live edge and walks left as
    /// it falls behind, measured against the whole buffer rather than the
    /// part of it that has filled.
    #[test]
    fn the_playhead_sits_where_the_tape_has_been_heard() {
        assert!(shift_progress(&tape(0.0, 600.0, 600.0)) == 1.0);
        assert!(shift_progress(&tape(150.0, 600.0, 600.0)) == 0.75);
        assert!(shift_progress(&tape(600.0, 600.0, 600.0)) == 0.0);

        // Half a buffer's worth taped: the edge is still the right end and
        // the oldest second held is the middle of the strip, which is where
        // the head stops rather than running on into the lead-in.
        assert!(shift_progress(&tape(0.0, 300.0, 600.0)) == 1.0);
        assert!(shift_progress(&tape(300.0, 300.0, 600.0)) == 0.5);
        assert!(shift_progress(&tape(400.0, 300.0, 600.0)) == 0.5);

        // A buffer of no length at all is the live edge and nothing else,
        // rather than a division by zero.
        assert!(shift_progress(&tape(0.0, 0.0, 0.0)) == 1.0);
    }

    /// How much of the strip is tape: the bar fills leftwards towards the
    /// cap instead of the strip growing as the connection tapes.
    #[test]
    fn the_tape_fills_the_bar_rather_than_growing_it() {
        assert!(shift_held(&tape(0.0, 0.0, 600.0)) == 0.0);
        assert!(shift_held(&tape(0.0, 300.0, 600.0)) == 0.5);
        assert!(shift_held(&tape(0.0, 600.0, 600.0)) == 1.0);
        assert!(shift_held(&tape(0.0, 0.0, 0.0)) == 1.0);
    }

    /// A fraction along the strip reads back as a distance from the live
    /// edge, which is what a click and a drag seek to.
    #[test]
    fn a_fraction_maps_back_to_a_distance_from_live() {
        let full = tape(0.0, 600.0, 600.0);
        assert!(shift_behind(1.0, &full) == 0.0);
        assert!(shift_behind(0.5, &full) == 300.0);
        assert!(shift_behind(0.0, &full) == 600.0);

        // Overshoot clamps rather than seeking past either end: a drag runs
        // off the strip all the time.
        assert!(shift_behind(1.5, &full) == 0.0);
        assert!(shift_behind(-0.5, &full) == 600.0);

        // The dashed lead-in is unreachable: anything left of the tape's
        // start lands on the oldest second there is.
        let half = tape(0.0, 300.0, 600.0);
        assert!(shift_behind(0.5, &half) == 300.0);
        assert!(shift_behind(0.25, &half) == 300.0);
        assert!(shift_behind(0.0, &half) == 300.0);
    }

    /// The ending slot leaves itself to the LIVE mark at the edge, and
    /// counts back to it in whole floored seconds once the playhead is
    /// behind. The edge is exact: the engine snaps it there.
    #[test]
    fn the_ending_slot_counts_back_only_once_it_is_behind() {
        assert!(behind_clock(None, 1).is_none());
        assert!(behind_clock(Some(&tape(0.0, 600.0, 600.0)), 1).is_none());

        assert!(behind_clock(Some(&tape(0.4, 600.0, 600.0)), 1).as_deref() == Some("-0:00"));
        // Floored, never rounded: a value sitting on a half second can't
        // tick between two readings of itself.
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

    /// A station's song marks sit where their songs began, measured back
    /// from the live edge against the whole buffer, exactly as many as the
    /// tape published.
    #[test]
    fn song_marks_sit_where_the_tape_put_them() {
        let shift = tape(0.0, 300.0, 600.0);
        let marks = tape_marks(&[song(0.0), song(150.0), song(300.0)], &shift);
        assert!(marks.len() == 3);
        assert!(marks[0].fraction == 1.0);
        assert!(marks[1].fraction == 0.75);
        // The oldest second held is the tape's own start, half way along a
        // buffer that's half full.
        assert!(marks[2].fraction == 0.5);
        // The ids are the places in the list the readout looks the song up
        // by, not anything the engine hands out.
        assert!(marks[2].id == 2);

        // A buffer of no length has nowhere to put anything.
        assert!(tape_marks(&[song(0.0)], &tape(0.0, 0.0, 0.0)).is_empty());
    }

    /// The lead-in's look, dash and sweep ride in the layout, and a layout
    /// saved before any of them existed comes back as the still dashes
    /// everyone has now.
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

    /// The sweep's band walks the lead-in end to end and starts over: it's
    /// off the line at both ends of a trip, crests somewhere along it in
    /// between, and lifts the resting weight only where it reaches.
    #[test]
    fn the_sweep_walks_the_lead_in_and_starts_over() {
        let base = 0x33;
        // A trip's start and its end have the band clear of the line, so
        // both ends of it rest at the weight they'd have with no sweep.
        let start = Sweep::at(0.0);
        assert!(start.alpha(base, 0.5) == base);
        assert!(start.alpha(base, 1.0) == base);
        // Half way through, the crest is mid-line and the far end is still
        // untouched.
        let middle = Sweep::at(super::SWEEP_SECS / 2.0);
        assert!(middle.alpha(base, 0.5) > base);
        assert!(middle.alpha(base, 0.0) == base);
        // The crest's lift is the gain, and no piece of the line is ever
        // lifted further than that.
        let crest = f32::from(base) * (1.0 + super::SWEEP_GAIN);
        assert!(middle.alpha(base, 0.5) == crest as u8);
        // And the trip repeats: a whole period on is the same band again.
        assert!(Sweep::at(super::SWEEP_SECS).alpha(base, 0.5) == start.alpha(base, 0.5));
    }

    /// A layout with no fields decodes to the stock row, and the retired
    /// timings toggle still reads: off leaves the strip alone.
    #[test]
    fn legacy_timings_folds_into_the_item_list() {
        let config: SeekConfig = serde_json::from_str("{}").unwrap();
        assert!(config.items == SeekConfig::default().items);

        let config: SeekConfig = serde_json::from_str(r#"{"timings": false}"#).unwrap();
        assert!(config.items == vec![SeekItem::Strip]);
    }

    /// A layout with the list uses it as-is, duplicates dropped,
    /// and round-trips through a save.
    #[test]
    fn item_lists_read_ordered_and_deduped() {
        let config: SeekConfig =
            serde_json::from_str(r#"{"items": ["strip", "elapsed", "strip"]}"#).unwrap();
        assert!(config.items == vec![SeekItem::Strip, SeekItem::Elapsed]);

        // Uniqueness is per row: a copy on the other side of a break is
        // kept through the load, only same-row repeats collapse.
        let config: SeekConfig =
            serde_json::from_str(r#"{"items": ["elapsed", "break", "elapsed"]}"#).unwrap();
        assert!(config.items == vec![SeekItem::Elapsed, SeekItem::Break, SeekItem::Elapsed]);

        let saved = serde_json::to_value(&config).unwrap();
        let back: SeekConfig = serde_json::from_value(saved).unwrap();
        assert!(back.items == config.items);
    }

    /// A break reads from a layout and cuts the list into rows, with an
    /// empty side dropping its row instead of drawing one.
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

    /// The editor's rows keep the empty well a trailing break makes, and
    /// the join puts the breaks back exactly.
    #[test]
    fn editor_rows_keep_empties_and_rejoin() {
        let items = vec![SeekItem::Strip, SeekItem::Break];
        let rows = editor_rows(&items);
        assert!(rows == vec![vec![SeekItem::Strip], vec![]]);
        assert!(rows.join(&SeekItem::Break) == items);
    }
}
