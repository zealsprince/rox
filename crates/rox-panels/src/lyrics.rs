//! The lyrics panel: the current track's words, timed against playback
//! when the file has an LRC-style sheet, plain scrolling text when it
//! doesn't. Which track is per-view config through [`TrackSource`]. A synced
//! sheet highlights and follows the line under the playhead, and clicking a
//! timed line seeks to it.
//!
//! With the read head on, the active line is cut where the playhead has sung
//! to. An enhanced (A2) sheet times that per word; a line-synced sheet spreads
//! the text across the line's span.
//!
//! The synced sheet keeps a child for every line rather than a virtual list:
//! wrapped rows have no uniform stride to virtualize against. Rows away from
//! the viewport are spacers holding their last measured height.
//!
//! Editing happens in its own window, which hands its unsaved draft back here
//! on every keystroke so an offset nudge shows live. Lyrics aren't in the
//! library projection, so a save just re-reads.
//!
//! A sheet is filed under a [`Subject`] rather than a path: a Subsonic song
//! under its server id, a radio song under the artist and title it announced.
//! A station's words only sync once we heard the song begin, so the first
//! song after tuning in reads as an unsynced sheet.

use std::ops::Range;
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    AnyElement, App, Axis, Bounds, Context, Div, EventEmitter, FocusHandle, Focusable, FontWeight,
    MouseButton, Pixels, ScrollDelta, ScrollHandle, ScrollWheelEvent, SharedString, Size,
    Subscription, WeakEntity, Window, canvas, div, prelude::*, px,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::spinner::Spinner;
use gpui_component::{Icon, Sizable};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::cue::TrackKey;
use rox_library::lyrics::{self, Lyrics, Subject, active_line, weave_rests};
use rox_services::lyrics::LyricsTarget;
use rox_services::player::song_clock;
use rox_viz::curve;
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::catalog::LibraryEvent;
use crate::design::{palette, tokens};
use crate::panel::{
    self, Align, AppState, PanelChrome, PanelSettings, ScrubState, align_row, items, justify,
};
use crate::panel_settings;
use crate::providers;
use crate::selection::SelectionEvent;
use crate::settings::lyrics_dir;
use crate::settings::ui as settings_ui;
use crate::source::{self, ResolvedTrack, TrackSource};

// Re-exported for the app's editor window and keymap.
pub use rox_panel_api::actions::StampLine;

const FONT_MIN: f32 = 8.0;
const FONT_MAX: f32 = 34.0;

/// Active-line fade-in length in seconds, and the opacity it starts from.
const FADE_SECS: f32 = 0.35;
const FADE_FLOOR: f32 = 0.15;

/// The fraction of a word's own slot its fade-in takes.
const WORD_FADE: f32 = 0.5;

/// How far a word rises as it fades in, as a fraction of the text size.
const WORD_RISE: f32 = 0.35;

/// How far past each viewport edge the synced sheet builds real rows, as a
/// fraction of the viewport. Enough that the glide never lands on a spacer.
const OVERSCAN: f32 = 0.5;

/// Wheel delta per lyric-line step. A wheel notch arrives as three lines.
const SCROLL_STEP_LINES: f32 = 3.0;

const GAP_MIN: f32 = 1.0;
const GAP_MAX: f32 = 20.0;

/// Below this height the empty face puts its line and search button on one row.
const EMPTY_INLINE_MAX_H: f32 = 120.0;

/// Auto-search only saves a match this confident; weaker ones wait for a look.
const AUTO_SAVE_CONFIDENCE: f32 = 0.9;

const SPACING_MIN: f32 = 1.2;
const SPACING_MAX: f32 = 3.0;

fn line_height(font: f32, spacing: f32) -> f32 {
    font * spacing
}

/// Which side of the active line the falloff dims.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DimEdge {
    Top,
    Bottom,
    #[default]
    Both,
}

impl DimEdge {
    fn dims(self, above: bool) -> bool {
        match self {
            DimEdge::Top => above,
            DimEdge::Bottom => !above,
            DimEdge::Both => true,
        }
    }
}

/// What a wordless line shows in the synced sheet.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RestMark {
    #[default]
    Note,
    Dots,
    None,
}

impl RestMark {
    fn str(self) -> &'static str {
        match self {
            RestMark::Note => "\u{266a}",
            RestMark::Dots => "\u{2026}",
            RestMark::None => "",
        }
    }
}

/// How the active line shows the read head: Fill sweeps a brightness
/// boundary across it, Build fades each word up as its turn comes.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WordStyle {
    #[default]
    Fill,
    Build,
}

/// The lyrics panel's per-view config, saved with the layout.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LyricsConfig {
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub source: TrackSource,
    pub align: Align,
    /// None inherits the app font. A missing family falls back at render.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    pub bold: bool,
    pub font_size: f32,
    /// Line height as a multiple of the text size.
    pub line_spacing: f32,
    /// Glide the active line to the middle as playback moves.
    pub follow: bool,
    /// Pad the synced list so the first and last lines can center too.
    pub pre_scroll: bool,
    pub fade_lines: bool,
    /// Run a read head through the active synced line.
    pub word_by_word: bool,
    pub word_style: WordStyle,
    /// Unsung dim under the fill: 0 leaves it full, 1 matches a passed line.
    pub word_dim: f32,
    /// Hide lines the playhead hasn't reached, revealing the sheet as it's sung.
    pub hide_upcoming: bool,
    pub wrap_lines: bool,
    /// Weave a rest before a first line that opens past [`Self::gap_secs`].
    pub intro_rest: bool,
    /// Weave a rest into instrumental gaps wider than [`Self::gap_secs`].
    pub gap_rest: bool,
    pub gap_secs: f32,
    /// Dim per step away from the active line, 0 to 1, compounding.
    pub dim: f32,
    pub dim_edge: DimEdge,
    /// Show the empty face's search button while a lyrics provider is on.
    pub search_button: bool,
    /// Search online for a track with no lyrics and save a confident match.
    pub auto_search: bool,
    /// Show the track's name on the empty face.
    pub show_name: bool,
    /// Pin the track's title above an unsynced sheet.
    pub show_title: bool,
    pub rest_mark: RestMark,
}

impl Default for LyricsConfig {
    fn default() -> Self {
        LyricsConfig {
            chrome: PanelChrome::default(),
            source: TrackSource::default(),
            align: Align::Center,
            font: None,
            bold: false,
            font_size: 18.0,
            line_spacing: 1.9,
            follow: true,
            pre_scroll: true,
            fade_lines: false,
            word_by_word: false,
            word_style: WordStyle::default(),
            word_dim: 0.6,
            hide_upcoming: false,
            wrap_lines: true,
            intro_rest: false,
            gap_rest: false,
            gap_secs: 5.0,
            dim: 0.0,
            dim_edge: DimEdge::default(),
            search_button: true,
            auto_search: false,
            show_name: false,
            show_title: false,
            rest_mark: RestMark::default(),
        }
    }
}

/// The lyrics target and what it was built from. Building one resolves the
/// catalog, so it's held until the track or the announced song moves.
struct TargetCache {
    key: TrackKey,
    /// Station-title revision at build time; comparing it skips the title lock.
    rev: u64,
    /// None for a station between announcements.
    built: Option<LyricsTarget>,
}

pub struct LyricsPanel {
    state: AppState,
    config: LyricsConfig,
    /// The loaded sheet and its "no lyrics" mark, keyed by subject so a
    /// station's next song misses instead of keeping the first song's words.
    loaded: Option<(Subject, Option<Arc<Lyrics>>, bool)>,
    pending: Option<Subject>,
    /// The edit window's unsaved draft, shown over the stored sheet while it's open.
    preview: Option<(Subject, Arc<Lyrics>)>,
    target: Option<TargetCache>,
    generation: u64,
    resolved: ResolvedTrack,
    /// The loaded sheet with rests woven in, keyed by the raw sheet's pointer
    /// and the rest-knob signature.
    display: Option<((usize, u64), Arc<Lyrics>)>,
    /// Indexes the woven [`Self::display`] lines.
    active_line: Option<usize>,
    faded_line: Option<usize>,
    active_fade: f32,
    /// The read head as a byte offset into the active line's text.
    head: Option<usize>,
    /// The playhead is on the shown track this render.
    positioned: bool,
    pad: Pixels,
    /// The synced sheet's scroll.
    wrap_scroll: ScrollHandle,
    /// Each line's last measured height, so an off-screen row can be a bare
    /// spacer. `heights_key` drops them on a resize or a size-knob change.
    heights: Vec<Pixels>,
    heights_key: Option<u64>,
    glide_to: Option<usize>,
    last_tick: Instant,
    scroll_accum: f32,
    /// The unsynced sheet's scroll.
    text_scroll: ScrollHandle,
    size_scrub: ScrubState,
    spacing_scrub: ScrubState,
    word_dim_scrub: ScrubState,
    dim_scrub: ScrubState,
    gap_scrub: ScrubState,
    value_edit: panel::ValueEdit,
    empty_size: Size<Pixels>,
    /// The subject auto-search last fired for, so it runs once per track. A
    /// subject, so each song on a station gets its own look.
    auto_tried: Option<Subject>,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
    _selection_changed: Subscription,
    _library_changed: Subscription,
}

impl LyricsPanel {
    pub fn new(state: AppState, config: LyricsConfig, cx: &mut Context<Self>) -> Self {
        let _player_changed = cx.observe(&state.player, |this: &mut Self, _, cx| {
            if this.tick_wakes(cx) {
                cx.notify();
            }
        });
        let _selection_changed = cx.subscribe(
            &state.selection,
            |this: &mut Self, _, _: &SelectionEvent, cx| {
                this.resolved.invalidate();
                cx.notify();
            },
        );
        // A rescan can rewrite tags and id -> path mappings.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.resolved.invalidate();
                this.loaded = None;
                this.target = None;
                cx.notify();
            },
        );
        // A save from any panel's editor reloads this one too.
        rox_panel_api::openers::lyrics_watch(cx.weak_entity().into(), cx);
        LyricsPanel {
            state,
            config,
            loaded: None,
            pending: None,
            preview: None,
            target: None,
            generation: 0,
            resolved: ResolvedTrack::default(),
            display: None,
            active_line: None,
            faded_line: None,
            active_fade: 1.0,
            head: None,
            positioned: false,
            pad: px(0.),
            wrap_scroll: ScrollHandle::new(),
            heights: Vec::new(),
            heights_key: None,
            glide_to: None,
            last_tick: Instant::now(),
            scroll_accum: 0.0,
            text_scroll: ScrollHandle::new(),
            size_scrub: ScrubState::default(),
            spacing_scrub: ScrubState::default(),
            word_dim_scrub: ScrubState::default(),
            dim_scrub: ScrubState::default(),
            gap_scrub: ScrubState::default(),
            value_edit: panel::ValueEdit::default(),
            empty_size: Size::default(),
            auto_tried: None,
            focus: cx.focus_handle().tab_stop(true),
            tab_panel: None,
            _player_changed,
            _selection_changed,
            _library_changed,
        }
    }

    /// The station-title revision, or zero unless the shown track is the live
    /// stream playing.
    fn live_rev(&self, key: &TrackKey, cx: &App) -> u64 {
        let player = self.state.player.read(cx);
        match player.now_playing() {
            Some(now) if now.live && now.key == *key => player.title_rev().unwrap_or(0),
            _ => 0,
        }
    }

    /// What the panel files and looks a sheet up under, cached against the
    /// track and the announced song. None for a station that hasn't named one.
    fn target(&mut self, key: &TrackKey, cx: &App) -> Option<&LyricsTarget> {
        // The revision is an atomic, the title a lock and two string clones, so
        // only read the title when the revision moved.
        let rev = self.live_rev(key, cx);
        if self.target.as_ref().map(|cache| (&cache.key, cache.rev)) != Some((key, rev)) {
            let song = (rev > 0)
                .then(|| self.state.player.read(cx).live_title())
                .flatten();
            let built =
                rox_services::lyrics::target_for(&self.state.library, key, song.as_ref(), cx);
            self.target = Some(TargetCache {
                key: key.clone(),
                rev,
                built,
            });
        }

        self.target.as_ref().and_then(|cache| cache.built.as_ref())
    }

    /// Whether a timed sheet can follow this track. A station only once we
    /// heard the song begin: tuning in mid-song leaves the stamps unanchored.
    fn can_sync(&self, key: &TrackKey, cx: &App) -> bool {
        match self.state.player.read(cx).now_playing() {
            Some(now) if now.live && now.key == *key => now.song_from_start,
            _ => true,
        }
    }

    /// The shown track's tags with a station's announced song laid over them.
    /// Read titles through here, or a stream shows the station's name.
    fn live_meta(&self, key: &TrackKey, cx: &App) -> Option<rox_library::store::TrackMeta> {
        let row = self.state.library.read(cx).meta_for_key(key);
        let player = self.state.player.read(cx);
        match player.now_playing() {
            Some(now) if now.key == *key => player.live_over(row),
            _ => row,
        }
    }

    /// Load `subject`'s lyrics off the UI thread unless cached or pending.
    fn ensure_loaded(&mut self, subject: &Subject, cx: &mut Context<Self>) {
        if self.loaded.as_ref().map(|(s, ..)| s) == Some(subject)
            || self.pending.as_ref() == Some(subject)
        {
            return;
        }
        self.pending = Some(subject.clone());
        self.generation += 1;
        let generation = self.generation;
        let subject = subject.clone();
        cx.spawn(async move |this, cx| {
            let (loaded, marked) = cx
                .background_executor()
                .spawn({
                    let subject = subject.clone();
                    async move {
                        let dir = lyrics_dir();
                        (
                            lyrics::load(&subject, Some(&dir)).map(Arc::new),
                            lyrics::marked_none(&subject, Some(&dir)),
                        )
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending = None;
                if this.loaded.as_ref().map(|(s, ..)| s) != Some(&subject) {
                    this.rewind();
                }
                this.loaded = Some((subject, loaded, marked));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn rewind(&mut self) {
        self.wrap_scroll.set_offset(Default::default());
        self.text_scroll.set_offset(Default::default());
        self.glide_to = None;
    }

    /// The edit window's draft while one is open on `subject`, else the loaded sheet.
    fn lyrics_for(&self, subject: &Subject) -> Option<&Arc<Lyrics>> {
        if let Some((at, draft)) = &self.preview
            && at == subject
        {
            return Some(draft);
        }

        self.loaded
            .as_ref()
            .filter(|(s, ..)| s == subject)
            .and_then(|(_, lyrics, _)| lyrics.as_ref())
    }

    /// From the last load, so the empty face does no IO per frame.
    fn marked_for(&self, subject: &Subject) -> bool {
        self.loaded
            .as_ref()
            .is_some_and(|(s, _, marked)| s == subject && *marked)
    }

    /// `raw` with the configured rests woven in, cached until the sheet or a
    /// rest knob changes.
    fn display_lyrics(&mut self, raw: &Arc<Lyrics>) -> Arc<Lyrics> {
        let key = (Arc::as_ptr(raw) as usize, self.rest_sig());
        if let Some((cached, lyrics)) = &self.display
            && *cached == key
        {
            return lyrics.clone();
        }
        let woven = weave_rests(
            raw,
            self.config.intro_rest,
            self.config.gap_rest,
            self.config.gap_secs as f64,
        );
        self.display = Some((key, woven.clone()));
        woven
    }

    fn display_arc(&self) -> Option<&Arc<Lyrics>> {
        self.display.as_ref().map(|(_, lyrics)| lyrics)
    }

    fn rest_sig(&self) -> u64 {
        let mut sig = 0u64;
        if self.config.intro_rest {
            sig |= 1;
        }
        if self.config.gap_rest {
            sig |= 2;
        }
        sig | ((self.config.gap_secs.to_bits() as u64) << 32)
    }

    fn config_menu(
        &self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu = source::source_flyout(
            menu,
            |this: &Self| this.config.source,
            &cx.entity(),
            |this, source, cx| {
                this.config.source = source;
                cx.notify();
            },
            window,
            cx,
        );
        let weak = cx.entity().downgrade();
        menu.separator().item(
            PopupMenuItem::new(rox_i18n::t!("lyrics-follow-playback"))
                .checked(self.config.follow)
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| {
                        this.config.follow = !this.config.follow;
                        cx.notify();
                    });
                }),
        )
    }

    fn open_edit(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.resolved.get(self.config.source, &self.state, cx) else {
            return;
        };
        let Some(target) = self.target(&key, cx).cloned() else {
            return;
        };
        rox_panel_api::openers::lyrics_edit(self.state.clone(), target, cx);
    }

    /// The timestamp `steps` timed lines from the active one, clamped to the
    /// ends. None when the step runs off the top during the intro.
    fn walk_lines(&self, steps: i32) -> Option<f64> {
        // The woven sheet, so a step can land on the rests too.
        let lyrics = self.display_arc()?;
        let timed: Vec<f64> = lyrics.lines.iter().filter_map(|line| line.at).collect();
        if timed.is_empty() {
            return None;
        }
        // -1 during the intro, before the first line lights.
        let cursor = self
            .active_line
            .map(|active| {
                lyrics.lines[..=active]
                    .iter()
                    .filter(|line| line.at.is_some())
                    .count() as i32
                    - 1
            })
            .unwrap_or(-1);
        let target = cursor + steps;
        if target < 0 {
            return None;
        }
        Some(timed[(target as usize).min(timed.len() - 1)])
    }

    /// Where playback is within `key`, or None when something else is playing.
    /// The whole key, so two tracks of one cue image read as different.
    fn playback_position(&self, key: &TrackKey, cx: &App) -> Option<f64> {
        self.state
            .player
            .read(cx)
            .now_playing()
            .filter(|now| now.key == *key)
            // A station's clock counts the listen; a sheet is timed from the song's top.
            .map(|now| song_clock(now.position_secs, now.song_start_secs))
    }

    /// Whether a pump tick is worth a repaint: only on a track turnover or when
    /// the lit line moves. The fade, build, and glide request their own frames.
    fn tick_wakes(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(key) = self.resolved.get(self.config.source, &self.state, cx) else {
            return self.loaded.is_some() || self.pending.is_some();
        };
        let Some(subject) = self.target(&key, cx).map(|t| t.subject.clone()) else {
            return self.loaded.is_some() || self.pending.is_some();
        };
        // Let the render kick the fetch, then wait for the load's own notify.
        if self.loaded.as_ref().map(|(s, ..)| s) != Some(&subject) {
            return self.pending.as_ref() != Some(&subject);
        }
        let Some(lyrics) = self.lyrics_for(&subject).cloned() else {
            return false;
        };
        if !lyrics.synced || !self.can_sync(&key, cx) {
            return false;
        }
        // The woven sheet, so the index matches what the render stores.
        let lyrics = self.display_lyrics(&lyrics);
        let active = self
            .playback_position(&key, cx)
            .and_then(|secs| active_line(&lyrics, secs));
        active != self.active_line
    }

    /// Open the match window. Nothing is written until the user confirms a pick.
    fn open_match(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.resolved.get(self.config.source, &self.state, cx) else {
            return;
        };
        let Some(target) = self.target(&key, cx).cloned() else {
            return;
        };
        rox_panel_api::openers::lyrics_matcher(self.state.clone(), target, cx);
    }

    /// Clear the shown track's lyrics everywhere and mark it as having none.
    /// Clearing alone would let the next lookup refill it.
    fn wipe(&mut self, cx: &mut Context<Self>) {
        self.set_none(true, cx);
    }

    fn unmark_none(&mut self, cx: &mut Context<Self>) {
        self.set_none(false, cx);
    }

    /// Wipes before marking, so a failed delete never leaves a marked track
    /// with words in it.
    fn set_none(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(key) = self.resolved.get(self.config.source, &self.state, cx) else {
            return;
        };
        let Some(subject) = self.target(&key, cx).map(|t| t.subject.clone()) else {
            return;
        };
        // Lifting the mark hands the subject back to auto-search.
        if !on && self.auto_tried.as_ref() == Some(&subject) {
            self.auto_tried = None;
        }
        cx.spawn(async move |_, cx| {
            let done = cx
                .background_executor()
                .spawn({
                    let subject = subject.clone();
                    async move {
                        let dir = lyrics_dir();
                        if on {
                            lyrics::wipe(&subject, Some(&dir))?;
                        }
                        lyrics::set_marked_none(&subject, &dir, on)
                    }
                })
                .await;
            if done.is_ok() {
                cx.update(|cx| rox_panel_api::openers::lyrics_saved(&subject, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// Drop the cached sheet for `subject` and repaint. Lyrics aren't in the
    /// projection, so the reload broadcast is the only signal to re-read.
    pub fn reload(&mut self, subject: &Subject, cx: &mut Context<Self>) {
        if self.loaded.as_ref().is_some_and(|(s, ..)| s == subject) {
            self.loaded = None;
        }
        cx.notify();
    }

    /// Take the edit window's unsaved draft for `subject`, or None when that
    /// window closed or moved to another track.
    pub fn set_preview(&mut self, subject: &Subject, text: Option<&str>, cx: &mut Context<Self>) {
        let held = self.preview.as_ref().map(|(at, _)| at.clone());
        if held.as_ref() != Some(subject) && text.is_none() {
            return;
        }
        let draft = text.map(|text| (subject.clone(), Arc::new(sheet(text.to_string()))));
        // Rewind only when a draft arrives or leaves, and only on this panel's
        // subject. Rewinding per keystroke would fight the reader for the scroll.
        let swapped = draft.as_ref().map(|(at, _)| at) != held.as_ref();
        if swapped && self.showing() == Some(subject) {
            self.rewind();
        }
        self.preview = draft;
        cx.notify();
    }

    /// The subject this panel is on, from the target cache the render fills.
    fn showing(&self) -> Option<&Subject> {
        self.target
            .as_ref()
            .and_then(|cache| cache.built.as_ref())
            .map(|target| &target.subject)
    }
}

impl PanelSettings for LyricsPanel {
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
        &[("Content", icons::FILE_TEXT)]
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
            .child(source::source_row(
                self.config.source,
                |this: &mut Self, source, cx| {
                    this.config.source = source;
                    cx.notify();
                },
                cx,
            ))
            .child(align_row(
                self.config.align,
                |this: &mut Self, align, cx| {
                    this.config.align = align;
                    cx.notify();
                },
                cx,
            ))
            .child(panel::setting_row(
                rox_i18n::t!("lyrics-follow-playback"),
                Some(rox_i18n::t!("lyrics-follow-playback.description")),
                panel::toggle(
                    self.config.follow,
                    |this: &mut Self, on, cx| {
                        this.config.follow = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("lyrics-always-centered"),
                Some(rox_i18n::t!("lyrics-always-centered.description")),
                panel::toggle(
                    self.config.pre_scroll,
                    |this: &mut Self, on, cx| {
                        this.config.pre_scroll = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("lyrics-fade-lines-in"),
                Some(rox_i18n::t!("lyrics-fade-lines-in.description")),
                panel::toggle(
                    self.config.fade_lines,
                    |this: &mut Self, on, cx| {
                        this.config.fade_lines = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("lyrics-build-word-by-word"),
                Some(rox_i18n::t!("lyrics-build-word-by-word.description")),
                panel::toggle(
                    self.config.word_by_word,
                    |this: &mut Self, on, cx| {
                        this.config.word_by_word = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("lyrics-read-head-style"),
                Some(rox_i18n::t!("lyrics-read-head-style.description")),
                panel::choices_shared(
                    &[
                        (rox_i18n::t!("lyrics-head-fill"), WordStyle::Fill),
                        (rox_i18n::t!("lyrics-head-build"), WordStyle::Build),
                    ],
                    self.config.word_style,
                    |this: &mut Self, style, cx| {
                        this.config.word_style = style;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("lyrics-unsung-dim"),
                Some(rox_i18n::t!("lyrics-unsung-dim.description")),
                settings_ui::scalar(
                    &self.word_dim_scrub,
                    &self.value_edit,
                    self.config.word_dim * 100.0,
                    settings_ui::span(0., 100., "%").hard(),
                    |this: &mut Self, value, cx| {
                        this.config.word_dim = value / 100.0;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("lyrics-hide-upcoming"),
                Some(rox_i18n::t!("lyrics-hide-upcoming.description")),
                panel::toggle(
                    self.config.hide_upcoming,
                    |this: &mut Self, on, cx| {
                        this.config.hide_upcoming = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("lyrics-wrap-lines"),
                Some(rox_i18n::t!("lyrics-wrap-lines.description")),
                panel::toggle(
                    self.config.wrap_lines,
                    |this: &mut Self, on, cx| {
                        this.config.wrap_lines = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("lyrics-lead-in-rest"),
                Some(rox_i18n::t!("lyrics-lead-in-rest.description")),
                panel::toggle(
                    self.config.intro_rest,
                    |this: &mut Self, on, cx| {
                        this.config.intro_rest = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("lyrics-rest-in-gaps"),
                Some(rox_i18n::t!("lyrics-rest-in-gaps.description")),
                panel::toggle(
                    self.config.gap_rest,
                    |this: &mut Self, on, cx| {
                        this.config.gap_rest = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .when(self.config.intro_rest || self.config.gap_rest, |d| {
                d.child(panel::setting_row(
                    rox_i18n::t!("lyrics-gap-threshold"),
                    Some(rox_i18n::t!("lyrics-gap-threshold.description")),
                    settings_ui::scalar(
                        &self.gap_scrub,
                        &self.value_edit,
                        self.config.gap_secs,
                        settings_ui::span(GAP_MIN, GAP_MAX, "s"),
                        |this: &mut Self, value, cx| {
                            this.config.gap_secs = value;
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .child(panel::setting_row(
                rox_i18n::t!("lyrics-line-falloff"),
                Some(rox_i18n::t!("lyrics-line-falloff.description")),
                settings_ui::scalar(
                    &self.dim_scrub,
                    &self.value_edit,
                    self.config.dim * 100.0,
                    settings_ui::span(0., 100., "%").hard(),
                    |this: &mut Self, value, cx| {
                        this.config.dim = value / 100.0;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("lyrics-falloff-edge"),
                Some(rox_i18n::t!("lyrics-falloff-edge.description")),
                panel::choices_shared(
                    &[
                        (rox_i18n::t!("lyrics-edge-top"), DimEdge::Top),
                        (rox_i18n::t!("lyrics-edge-bottom"), DimEdge::Bottom),
                        (rox_i18n::t!("choice-both"), DimEdge::Both),
                    ],
                    self.config.dim_edge,
                    |this: &mut Self, edge, cx| {
                        this.config.dim_edge = edge;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("lyrics-search-button"),
                Some(rox_i18n::t!("lyrics-search-button.description")),
                panel::toggle(
                    self.config.search_button,
                    |this: &mut Self, on, cx| {
                        this.config.search_button = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("lyrics-auto-search"),
                Some(rox_i18n::t!("lyrics-auto-search.description")),
                panel::toggle(
                    self.config.auto_search,
                    |this: &mut Self, on, cx| {
                        this.config.auto_search = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("lyrics-show-song-name"),
                Some(rox_i18n::t!("lyrics-show-song-name.description")),
                panel::toggle(
                    self.config.show_name,
                    |this: &mut Self, on, cx| {
                        this.config.show_name = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("lyrics-title-unsynced"),
                Some(rox_i18n::t!("lyrics-title-unsynced.description")),
                panel::toggle(
                    self.config.show_title,
                    |this: &mut Self, on, cx| {
                        this.config.show_title = on;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                rox_i18n::t!("lyrics-rest-marker"),
                Some(rox_i18n::t!("lyrics-rest-marker.description")),
                panel::choices_shared(
                    &[
                        (rox_i18n::t!("lyrics-mark-note"), RestMark::Note),
                        (rox_i18n::t!("lyrics-mark-dots"), RestMark::Dots),
                        (rox_i18n::t!("shader-pick-none"), RestMark::None),
                    ],
                    self.config.rest_mark,
                    |this: &mut Self, mark, cx| {
                        this.config.rest_mark = mark;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .into_any_element()
    }

    // The Appearance section below has its own font picker.
    fn has_own_font(&self) -> bool {
        true
    }

    fn appearance(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let reset = settings_ui::small_button(
            rox_i18n::t!("panel-reset"),
            icons::REFRESH_CW,
            false,
            cx.listener(|this, _, _, cx| {
                let default = LyricsConfig::default();
                this.config.font = default.font;
                this.config.bold = default.bold;
                this.config.font_size = default.font_size;
                this.config.line_spacing = default.line_spacing;
                cx.notify();
            }),
        );
        Some(
            settings_ui::section(
                rox_i18n::t!("lyrics-title"),
                Some(reset.into_any_element()),
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(panel::setting_row(
                        rox_i18n::t!("panel-font"),
                        Some(rox_i18n::t!("lyrics-font.description")),
                        panel::font_picker(
                            "lyrics-font",
                            self.config.font.clone(),
                            |this: &mut Self, font, cx| {
                                this.config.font = font;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(panel::setting_row(
                        rox_i18n::t!("lyrics-bold"),
                        None,
                        panel::toggle(
                            self.config.bold,
                            |this: &mut Self, on, cx| {
                                this.config.bold = on;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(panel::setting_row(
                        rox_i18n::t!("lyrics-text-size"),
                        Some(rox_i18n::t!("lyrics-text-size.description")),
                        settings_ui::scalar(
                            &self.size_scrub,
                            &self.value_edit,
                            self.config.font_size,
                            settings_ui::span(FONT_MIN, FONT_MAX, "px"),
                            |this: &mut Self, value, cx| {
                                this.config.font_size = value;
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(panel::setting_row(
                        rox_i18n::t!("lyrics-line-spacing"),
                        Some(rox_i18n::t!("lyrics-line-spacing.description")),
                        settings_ui::scalar(
                            &self.spacing_scrub,
                            &self.value_edit,
                            self.config.line_spacing,
                            settings_ui::span(SPACING_MIN, SPACING_MAX, "x").decimals(1),
                            |this: &mut Self, value, cx| {
                                this.config.line_spacing = value;
                                cx.notify();
                            },
                            cx,
                        ),
                    )),
            )
            .into_any_element(),
        )
    }
}

impl EventEmitter<PanelEvent> for LyricsPanel {}

impl Focusable for LyricsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for LyricsPanel {
    fn panel_name(&self) -> &'static str {
        "lyrics"
    }

    rox_panel_api::opens_settings!();

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(
            self.config.chrome.title.as_deref(),
            rox_i18n::t!("lyrics-title"),
        )
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    fn title_suffix(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        // No pencil for a station between announcements.
        let key = self.resolved.get(self.config.source, &self.state, cx)?;
        self.target(&key, cx)?;
        let weak = cx.entity().downgrade();
        Some(settings_ui::icon_button(
            icons::PENCIL,
            false,
            move |_, _, cx| {
                let Some(this) = weak.upgrade() else { return };
                this.update(cx, |this, cx| this.open_edit(cx));
            },
        ))
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
            gpui::size(
                rox_dock::resizable::PANEL_MIN_SIZE,
                rox_dock::resizable::PANEL_MIN_SIZE,
            ),
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let menu = self.config_menu(menu, window, cx);
        let menu = menu.separator();
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new(rox_i18n::t!("lyrics-edit-lyrics"))
                .icon(Icon::default().path(icons::PENCIL))
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| this.open_edit(cx));
                }),
        );
        let menu = if providers::lyrics_online() {
            let weak = cx.entity().downgrade();
            menu.item(
                PopupMenuItem::new(rox_i18n::t!("lyrics-find-online"))
                    .icon(Icon::default().path(icons::DOWNLOAD))
                    .on_click(move |_, _, cx| {
                        let Some(this) = weak.upgrade() else { return };
                        this.update(cx, |this, cx| this.open_match(cx));
                    }),
            )
        } else {
            menu
        };
        // Wipe and the no-lyrics mark read as one switch: a marked track loads
        // empty, so a showing sheet is never marked.
        let subject = self
            .resolved
            .get(self.config.source, &self.state, cx)
            .and_then(|key| self.target(&key, cx))
            .map(|target| target.subject.clone());
        let menu = match subject {
            Some(subject) if self.lyrics_for(&subject).is_some() => {
                let weak = cx.entity().downgrade();
                menu.item(
                    PopupMenuItem::new(rox_i18n::t!("lyrics-wipe-lyrics"))
                        .icon(Icon::default().path(icons::TRASH))
                        .on_click(move |_, _, cx| {
                            let Some(this) = weak.upgrade() else { return };
                            this.update(cx, |this, cx| this.wipe(cx));
                        }),
                )
            }
            Some(subject) => {
                let marked = lyrics::marked_none(&subject, Some(&lyrics_dir()));
                let weak = cx.entity().downgrade();
                menu.item(
                    PopupMenuItem::new(rox_i18n::t!("lyrics-no-lyrics-track"))
                        .icon(Icon::default().path(icons::MINUS))
                        .checked(marked)
                        .on_click(move |_, _, cx| {
                            let Some(this) = weak.upgrade() else { return };
                            this.update(cx, |this, cx| {
                                if marked {
                                    this.unmark_none(cx);
                                } else {
                                    this.wipe(cx);
                                }
                            });
                        }),
                )
            }
            None => menu,
        };
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
                LyricsPanel::new(state, config, cx)
            },
        );
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
            window,
        )
    }
}

impl Render for LyricsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        // A focus stop, which also puts the tab group on the focus path for the
        // tab-cycle chord.
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(window, cx).track_focus(&focus))
    }
}

impl LyricsPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // Solo there's no tab bar for the pencil, and the right-click menu opens
        // the editor instead. Don't add a body toolbar just to hold it.
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_root())
            .when_some(self.config.font.clone(), |d, font| d.font_family(font))
            .when(self.config.bold, |d| d.font_weight(FontWeight::BOLD))
            .child(self.content(window, cx).flex_1().min_h_0())
    }

    fn content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(key) = self.resolved.get(self.config.source, &self.state, cx) else {
            return if self.config.show_name {
                div().size_full()
            } else {
                placeholder(rox_i18n::t!("content-no-track"))
            };
        };

        // The subject, not the key: a stream's song turns over under one URL.
        let Some(subject) = self.target(&key, cx).map(|t| t.subject.clone()) else {
            return self.empty_face(&key, None, cx);
        };

        self.ensure_loaded(&subject, cx);
        let Some(lyrics) = self.lyrics_for(&subject).cloned() else {
            return if self.pending.as_ref() == Some(&subject) {
                loading()
            } else {
                self.empty_face(&key, Some(&subject), cx)
            };
        };

        if lyrics.synced && self.can_sync(&key, cx) {
            self.synced_face(&key, &lyrics, window, cx)
        } else {
            self.plain_face(&key, &lyrics, cx)
        }
    }

    /// The empty face: the "no lyrics" line with the online search under it,
    /// or beside it once the panel is too short. A marked track offers the way
    /// back out instead, since the mark is what stops the lookups.
    fn empty_face(
        &mut self,
        key: &TrackKey,
        subject: Option<&Subject>,
        cx: &mut Context<Self>,
    ) -> Div {
        if let Some(subject) = subject {
            self.maybe_auto_search(subject, cx);
        }
        let align = self.config.align;
        // Only a measured, short panel goes inline, so the first frame never flickers.
        let inline =
            self.empty_size.height > px(0.) && self.empty_size.height < px(EMPTY_INLINE_MAX_H);
        let marked = subject.is_some_and(|subject| self.marked_for(subject));
        // No subject is a station between announcements, with no song to search for.
        let show_button =
            self.config.search_button && providers::lyrics_online() && subject.is_some();
        let button = show_button.then(|| {
            if marked {
                settings_ui::small_button(
                    rox_i18n::t!("lyrics-look-again"),
                    icons::REFRESH_CW,
                    false,
                    cx.listener(|this, _, _, cx| this.unmark_none(cx)),
                )
            } else {
                settings_ui::small_button(
                    rox_i18n::t!("lyrics-search-online"),
                    icons::DOWNLOAD,
                    false,
                    cx.listener(|this, _, _, cx| this.open_match(cx)),
                )
            }
        });
        let name = self
            .config
            .show_name
            .then(|| self.track_name(key, cx))
            .filter(|name| !name.is_empty());

        let face = div()
            .size_full()
            .relative()
            .flex()
            .gap(tokens::SPACE_SM)
            .p(tokens::SPACE_MD);
        // A row aligns along its main axis, a column along the cross axis.
        let face = if inline {
            justify(face.flex_row().items_center(), align)
        } else {
            items(face.flex_col().justify_center(), align)
        };
        let show_notice = name.is_none();
        face.when_some(name, |d, name| {
            d.child(
                div()
                    .max_w_full()
                    .truncate()
                    .text_color(palette::text_bright())
                    .child(name),
            )
        })
        .when(show_notice, |d| {
            let notice = if marked {
                rox_i18n::t!("lyrics-marked-notice")
            } else {
                rox_i18n::t!("lyrics-no-lyrics-notice")
            };
            d.child(div().text_color(palette::text_faint()).child(notice))
        })
        .when_some(button, |d, button| d.child(button))
        // Reports the face's size so the next frame can pick stacked or inline.
        .child(
            canvas(
                {
                    let weak = cx.entity().downgrade();
                    move |bounds: Bounds<Pixels>, _, cx| {
                        if let Some(this) = weak.upgrade() {
                            this.update(cx, |this, cx| {
                                if this.empty_size != bounds.size {
                                    this.empty_size = bounds.size;
                                    cx.notify();
                                }
                            });
                        }
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
    }

    /// Title and artist, the file stem standing in for a missing title.
    fn track_name(&self, key: &TrackKey, cx: &App) -> SharedString {
        let meta = self.live_meta(key, cx);
        let (title, artist) = meta.map(|m| (m.title, m.artist)).unwrap_or_default();
        let title = if title.is_empty() {
            key.path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            title
        };
        if artist.is_empty() {
            title.into()
        } else {
            format!("{title} - {artist}").into()
        }
    }

    /// Look the shown track up once and save the top match if it clears
    /// [`AUTO_SAVE_CONFIDENCE`]. Never runs on a marked track: the mark is there
    /// because a lookup got it wrong.
    fn maybe_auto_search(&mut self, subject: &Subject, cx: &mut Context<Self>) {
        if !self.config.auto_search || !providers::lyrics_online() {
            return;
        }
        if self.auto_tried.as_ref() == Some(subject) {
            return;
        }
        self.auto_tried = Some(subject.clone());
        if lyrics::marked_none(subject, Some(&lyrics_dir())) {
            return;
        }
        let Some(query) = self.target.as_ref().and_then(|cache| cache.built.as_ref()) else {
            return;
        };
        let query = query.query.clone();
        if query.artist.is_empty() || query.title.is_empty() {
            return;
        }
        let subject = subject.clone();
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn({
                    let subject = subject.clone();
                    async move {
                        let found = providers::search_lyrics(&query).ok()?;
                        let best = found.into_iter().next()?;
                        if best.confidence < AUTO_SAVE_CONFIDENCE {
                            return None;
                        }
                        let target = rox_services::lyrics::save_target(&subject);
                        lyrics::save(&subject, &target, &best.text, Some(&lyrics_dir())).ok()
                    }
                })
                .await;
            if saved.is_none() {
                return;
            }
            this.update(cx, |this, cx| this.reload(&subject, cx)).ok();
        })
        .detach();
    }

    /// The synced face: one row per timed line, the active one lit and followed.
    fn synced_face(
        &mut self,
        key: &TrackKey,
        lyrics: &Arc<Lyrics>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        // The woven sheet: rests are real timed lines to the follow and highlight.
        let lyrics = self.display_lyrics(lyrics);
        let position = self.playback_position(key, cx);
        let active = position.and_then(|secs| active_line(&lyrics, secs));
        self.active_line = active;
        self.positioned = position.is_some();

        let dt = self.last_tick.elapsed().as_secs_f32().min(0.05);
        self.last_tick = Instant::now();
        let mut animating = false;

        if self.config.fade_lines {
            if self.faded_line != active {
                self.faded_line = active;
                self.active_fade = 0.0;
            }
            if self.active_fade < 1.0 {
                self.active_fade = (self.active_fade + dt / FADE_SECS).min(1.0);
                animating |= self.active_fade < 1.0;
            }
        } else {
            self.active_fade = 1.0;
        }

        self.head = None;
        if self.config.word_by_word
            && let (Some(pos), Some(ix)) = (position, active)
        {
            let line = &lyrics.lines[ix];
            let until = lyrics.lines[ix + 1..].iter().find_map(|line| line.at);
            self.head = Some(lyrics::read_head(line, pos, until));
        }
        // Keep frames coming while the head has line left, since the pump only
        // wakes on a line change. Not while paused: the player's change wakes it.
        animating |= self.state.player.read(cx).is_playing()
            && match (self.head, active) {
                (Some(head), Some(ix)) => head < lyrics.lines[ix].text.len(),
                _ => false,
            };

        self.pad = self.pad_height(line_height(self.config.font_size, self.config.line_spacing));

        if self.config.follow
            && let Some(active) = active
        {
            self.glide_to = Some(active);
        }
        if let Some(row) = self.glide_to {
            let arrived = match self.center_target(row) {
                Some(target) => {
                    !panel::glide_step_axis(&self.wrap_scroll, Axis::Vertical, target, dt)
                }
                // Nothing to measure before the first layout; retry next frame.
                None => false,
            };
            // A target past a shorter sheet's end would never arrive.
            if arrived || row >= lyrics.lines.len() {
                self.glide_to = None;
            } else {
                animating = true;
            }
        }
        if animating {
            window.request_animation_frame();
        }

        let pad = self.pad;
        let rows = self.line_rows(cx);
        div()
            .size_full()
            // With follow on the sheet never free-scrolls, so the wheel steps through
            // the sung lines and seeks to each. Off, or with no playhead, it scrolls.
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                if !this.config.follow || !this.positioned {
                    return;
                }
                let lines = match event.delta {
                    ScrollDelta::Lines(lines) => lines.y,
                    ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / 20.0,
                };
                if lines == 0.0 {
                    return;
                }
                // Bank the delta and spend it a line at a time; wheel down steps forward.
                this.scroll_accum += lines;
                let mut steps = 0i32;
                while this.scroll_accum <= -SCROLL_STEP_LINES {
                    this.scroll_accum += SCROLL_STEP_LINES;
                    steps += 1;
                }
                while this.scroll_accum >= SCROLL_STEP_LINES {
                    this.scroll_accum -= SCROLL_STEP_LINES;
                    steps -= 1;
                }
                if steps == 0 {
                    return;
                }
                if let Some(at) = this.walk_lines(steps) {
                    this.state.player.read(cx).seek_to(at);
                    cx.notify();
                }
            }))
            .child(
                div()
                    .id("lyrics-lines")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.wrap_scroll)
                    .flex()
                    .flex_col()
                    // The pads are children 0 and last, so line ix is child ix + 1. They stay
                    // at zero height with pre-scroll off so that offset never moves.
                    .child(div().flex_none().h(pad))
                    .children(rows)
                    .child(div().flex_none().h(pad)),
            )
    }

    /// Where the scroll should sit to center line `ix`, off the row's measured
    /// bounds, since wrapped rows have no uniform stride. None until measured.
    ///
    /// The handle records bounds during prepaint, before the scroll offset is
    /// applied, so they're content positions. Don't subtract the offset again:
    /// the glide feeds on its own output and runs off the end.
    fn center_target(&self, ix: usize) -> Option<Pixels> {
        let view = self.wrap_scroll.bounds();
        let item = self.wrap_scroll.bounds_for_item(ix + 1)?;
        let origin = item.top() - view.top();

        panel::glide_target_at(&self.wrap_scroll, Axis::Vertical, origin, item.size.height)
    }

    /// Half the viewport with pre-scroll on, so the end lines can center. A
    /// screenful of rows stands in before the first layout.
    fn pad_height(&self, line_h: f32) -> Pixels {
        if !self.config.pre_scroll {
            return px(0.);
        }
        let viewport = self.wrap_scroll.bounds().size.height;

        if viewport > px(0.) {
            viewport / 2.0
        } else {
            px(line_h * 12.0)
        }
    }

    /// Refresh the remembered row heights, dropping them when the sheet, the
    /// size knobs, wrapping, or the width changes.
    fn measure_rows(&mut self, lyrics: &Arc<Lyrics>) {
        let key = {
            let mut hash = std::collections::hash_map::DefaultHasher::new();
            use std::hash::{Hash, Hasher};
            (Arc::as_ptr(lyrics) as usize).hash(&mut hash);
            self.config.font_size.to_bits().hash(&mut hash);
            self.config.line_spacing.to_bits().hash(&mut hash);
            self.config.wrap_lines.hash(&mut hash);
            f32::from(self.wrap_scroll.bounds().size.width)
                .to_bits()
                .hash(&mut hash);
            hash.finish()
        };
        if self.heights_key != Some(key) {
            self.heights_key = Some(key);
            self.heights.clear();
        }

        // The pads are children 0 and last, so line ix is child ix + 1.
        let count = lyrics.lines.len();
        if self.heights.len() < count {
            self.heights.resize(count, px(0.));
        }
        for ix in 0..count {
            if let Some(bounds) = self.wrap_scroll.bounds_for_item(ix + 1) {
                self.heights[ix] = bounds.size.height;
            }
        }
    }

    /// The lines to build for real this frame: the viewport plus [`OVERSCAN`]
    /// either side. None asks for the whole sheet, the measuring pass before
    /// every row has a height.
    fn visible_lines(&self, count: usize) -> Option<Range<usize>> {
        let viewport = self.wrap_scroll.bounds().size.height;
        if viewport <= px(0.) || self.heights.len() < count {
            return None;
        }
        if self.heights.iter().take(count).any(|h| *h <= px(0.)) {
            return None;
        }

        // Content space, the frame the cached heights and glide target use.
        let scrolled = -self.wrap_scroll.offset().y;
        let from = scrolled - viewport * OVERSCAN;
        let to = scrolled + viewport * (1.0 + OVERSCAN);

        let mut first = count;
        let mut last = 0;
        let mut top = self.pad;
        for (ix, height) in self.heights.iter().take(count).enumerate() {
            if top + *height >= from && top <= to {
                first = first.min(ix);
                last = ix;
            }
            top += *height;
        }

        (first <= last).then_some(first..last + 1)
    }

    /// The synced sheet's rows. Every line gets a child so indices never move;
    /// only rows in [`Self::visible_lines`] get their words, the rest are spacers
    /// at their measured height.
    fn line_rows(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(lyrics) = self.display_arc().cloned() else {
            return Vec::new();
        };
        self.measure_rows(&lyrics);
        let built = self.visible_lines(lyrics.lines.len());
        let active = self.active_line;
        let align = self.config.align;
        let text_align = text_align(align);
        let font = self.config.font_size;
        let line_h = line_height(font, self.config.line_spacing);
        let fade = self.active_fade;
        let fade_lines = self.config.fade_lines;
        let head = self.head;
        let word_by_word = self.config.word_by_word;
        let word_style = self.config.word_style;
        let word_dim = self.config.word_dim;
        let wrap = self.config.wrap_lines;
        let positioned = self.positioned;
        let dim = self.config.dim;
        let dim_edge = self.config.dim_edge;
        let rest_mark = self.config.rest_mark.str();
        let unsung = palette::mix(palette::text_bright(), palette::text_muted(), word_dim);
        // Before the first line lights, measure the falloff from the first timed
        // line so a live intro is already dimmed. No playhead, no falloff.
        let falloff_from = active.or_else(|| {
            positioned.then(|| {
                lyrics
                    .lines
                    .iter()
                    .position(|line| line.at.is_some())
                    .unwrap_or(0)
            })
        });

        let heights = std::mem::take(&mut self.heights);
        let rows = (0..lyrics.lines.len())
            .map(|ix| {
                // Off-screen rows only hold their space: no words, no listener.
                if built.as_ref().is_some_and(|built| !built.contains(&ix)) {
                    return div()
                        .flex_none()
                        .h(heights.get(ix).copied().unwrap_or(px(line_h)))
                        .into_any_element();
                }
                let line = &lyrics.lines[ix];
                let at = line.at;
                let is_active = Some(ix) == active;
                let text = &line.text;
                let running = is_active && word_by_word && !text.is_empty();

                let content: AnyElement = match (running.then_some(head).flatten(), word_style) {
                    // One wrapping text layout with the tail highlighted. Two elements would
                    // break the wrap and stop the cut landing mid-word.
                    (Some(head), WordStyle::Fill) => gpui::StyledText::new(text.clone())
                        .with_highlights([(
                            head..text.len(),
                            gpui::HighlightStyle {
                                color: Some(unsung.into()),
                                ..Default::default()
                            },
                        )])
                        .into_any_element(),

                    (Some(head), WordStyle::Build) => {
                        let rise = font * WORD_RISE;
                        div()
                            .max_w_full()
                            .flex()
                            .flex_row()
                            .flex_wrap()
                            .children(word_spans(text).into_iter().map(|span| {
                                let opacity = crossed(head, &span);
                                div()
                                    .relative()
                                    .top(px((1.0 - opacity) * rise))
                                    .opacity(opacity)
                                    .child(SharedString::from(format!("{}\u{a0}", &text[span])))
                            }))
                            .into_any_element()
                    }

                    _ => div()
                        .max_w_full()
                        .when(!wrap, |d| d.truncate())
                        .child(if text.is_empty() {
                            SharedString::from(rest_mark)
                        } else {
                            SharedString::from(text.clone())
                        })
                        .into_any_element(),
                };

                let upcoming = self.config.hide_upcoming
                    && positioned
                    && active.is_none_or(|active| ix > active);

                let opacity = if upcoming {
                    0.0
                } else if is_active {
                    if fade_lines {
                        FADE_FLOOR + (1.0 - FADE_FLOOR) * fade
                    } else {
                        1.0
                    }
                } else {
                    falloff(dim, dim_edge, falloff_from, ix)
                };

                let row = justify(
                    div()
                        .w_full()
                        .flex()
                        .flex_none()
                        .items_center()
                        .min_h(px(line_h))
                        .when(!wrap, |d| d.h(px(line_h)).overflow_hidden()),
                    align,
                )
                .px(tokens::SPACE_MD)
                .text_size(px(font))
                .line_height(px(line_h))
                .text_align(text_align)
                .opacity(opacity)
                .text_color(if is_active {
                    palette::text_bright()
                } else {
                    palette::text_muted()
                })
                .child(content);
                // Clicking the playing line does nothing rather than seeking to its start.
                let row = row.when_some(at.filter(|_| !is_active), |d, at| {
                    d.cursor_pointer()
                        .hover(|d| d.text_color(palette::text_bright()))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.state.player.read(cx).seek_to(at);
                                cx.notify();
                            }),
                        )
                });
                row.into_any_element()
            })
            .collect();
        self.heights = heights;

        rows
    }

    /// The plain face for untimed lyrics, the title optionally pinned above.
    fn plain_face(&self, key: &TrackKey, lyrics: &Arc<Lyrics>, cx: &App) -> Div {
        let align = self.config.align;
        let text_align = match align {
            Align::Left => gpui::TextAlign::Left,
            Align::Center => gpui::TextAlign::Center,
            Align::Right => gpui::TextAlign::Right,
        };
        let font = self.config.font_size;
        let spacing = self.config.line_spacing;
        let body = div()
            .flex()
            .flex_col()
            .w_full()
            .p(tokens::SPACE_MD)
            .text_size(px(font))
            .line_height(px(font * spacing))
            .text_color(palette::text())
            .children(lyrics.lines.iter().map(|line| {
                if line.text.is_empty() {
                    div().h(px(font * spacing * 0.5))
                } else {
                    div()
                        .w_full()
                        .text_align(text_align)
                        .child(SharedString::from(line.text.clone()))
                }
            }));
        let title = self.config.show_title.then(|| self.track_name(key, cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .when_some(title, |d, title| {
                d.child(
                    div()
                        .flex_none()
                        .w_full()
                        .px(tokens::SPACE_MD)
                        .pt(tokens::SPACE_MD)
                        .pb(tokens::SPACE_SM)
                        .text_size(px(font))
                        .text_align(text_align)
                        .text_color(palette::text_bright())
                        .truncate()
                        .child(title),
                )
            })
            .child(
                div()
                    .id("lyrics-sheet")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.text_scroll)
                    // min_h_full plus justify_center centers a short sheet while a long one
                    // still scrolls from the top.
                    .child(
                        div()
                            .min_h_full()
                            .w_full()
                            .flex()
                            .flex_col()
                            .justify_center()
                            .child(body),
                    ),
            )
    }
}

/// A non-active line's opacity: `dim` compounding per step from the active
/// line, on the dimming edge only.
fn falloff(dim: f32, edge: DimEdge, active: Option<usize>, ix: usize) -> f32 {
    let Some(active) = active else {
        return 1.0;
    };
    if !edge.dims(ix < active) {
        return 1.0;
    }
    curve::falloff(dim, ix.abs_diff(active) as u32)
}

fn text_align(align: Align) -> gpui::TextAlign {
    match align {
        Align::Left => gpui::TextAlign::Left,
        Align::Center => gpui::TextAlign::Center,
        Align::Right => gpui::TextAlign::Right,
    }
}

/// Byte ranges of `text`'s words, so the read head's offset lines up.
fn word_spans(text: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut open: Option<usize> = None;
    for (ix, ch) in text.char_indices() {
        match (ch.is_whitespace(), open) {
            (false, None) => open = Some(ix),
            (true, Some(from)) => {
                spans.push(from..ix);
                open = None;
            }
            _ => {}
        }
    }
    if let Some(from) = open {
        spans.push(from..text.len());
    }
    spans
}

/// How far `head` has crossed `span`, 0 to 1, fading over the first
/// [`WORD_FADE`] of it.
fn crossed(head: usize, span: &Range<usize>) -> f32 {
    if head <= span.start {
        return 0.0;
    }
    if head >= span.end {
        return 1.0;
    }
    let through = (head - span.start) as f32 / (span.end - span.start).max(1) as f32;
    (through / WORD_FADE).clamp(0.0, 1.0)
}

/// Parse the edit window's draft the way [`lyrics::load`] would.
/// `Source::Tag` is a placeholder, since nothing saves from a draft.
fn sheet(text: String) -> Lyrics {
    let (lines, synced) = lyrics::parse(&text);

    Lyrics {
        source: lyrics::Source::Tag,
        text,
        lines,
        synced,
    }
}

fn placeholder(text: impl Into<SharedString>) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p(tokens::SPACE_MD)
        .text_color(palette::text_faint())
        .child(text.into())
}

fn loading() -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p(tokens::SPACE_MD)
        .text_color(palette::text_faint())
        .child(Spinner::new().with_size(gpui_component::Size::Small))
}
