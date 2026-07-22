//! The lyrics panel: the current track's words, timed against playback
//! when the file carries an LRC-style sheet, plain scrolling text when it
//! does not. Which track is per-view config through [`TrackSource`], the
//! same knob the cover and metadata panels carry, so a duplicate can watch
//! each. A synced sheet highlights the line under the playhead and, with
//! follow on, glides it to the middle the way the library's now-playing
//! row does; clicking a timed line seeks to it.
//!
//! The pencil in the title row opens the edit window (see [`crate::lyrics::edit`]):
//! the raw text becomes a multi-line input over a baseline read off the
//! file, and a save writes it back where it came from: the embedded tag
//! through the writer's atomic layer, or the `.lrc` sidecar or app lyrics
//! store as a plain file. Lyrics do not ride the library projection, so a
//! save just re-reads the file.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    actions, canvas, div, prelude::*, px, uniform_list, AnyElement, App, Bounds, Context, Div,
    EventEmitter, FocusHandle, Focusable, FontWeight, MouseButton, Pixels, ScrollDelta,
    ScrollHandle, ScrollWheelEvent, SharedString, Size, Subscription, UniformListScrollHandle,
    WeakEntity, Window,
};
use gpui_component::menu::{PopupMenu, PopupMenuItem};
use gpui_component::spinner::Spinner;
use gpui_component::{Icon, Sizable};
use rox_dock::{Panel, PanelEvent, TabPanel};
use rox_library::lyrics::{self, Line, Lyrics};
use serde::{Deserialize, Serialize};

use crate::assets::icons;
use crate::design::{palette, tokens};
use crate::panel::{
    self, align_row, items, justify, Align, AppState, PanelChrome, PanelSettings, ScrubState,
};
use crate::panel_settings;
use crate::panels::library::LibraryEvent;
use crate::providers;
use crate::selection::SelectionEvent;
use crate::settings::lyrics_dir;
use crate::settings_ui;
use crate::source::{self, ResolvedTrack, TrackSource};

actions!(
    lyrics,
    [
        /// Stamp the cursor's line with the current playback position and
        /// step to the next, bound to Shift+Enter while the editor is open.
        StampLine
    ]
);

/// The text-size slider's range, in px. The floor goes small enough for a
/// dense sheet packed into a narrow panel, the ceiling comfortably big for
/// an across-the-room karaoke view.
const FONT_MIN: f32 = 8.0;
const FONT_MAX: f32 = 34.0;

/// How long a line takes to fade in once it becomes active, in seconds,
/// and the opacity it starts that fade from.
const FADE_SECS: f32 = 0.35;
const FADE_FLOOR: f32 = 0.15;

/// How much of its own slot a word takes to fade fully in once the read
/// head reaches it, as a fraction of that slot. Smaller is snappier.
const WORD_FADE: f32 = 0.5;

/// How far a word rises into place as it fades in, as a fraction of the
/// text size.
const WORD_RISE: f32 = 0.35;

/// The assumed duration of a synced line with nothing timed after it, so
/// the last line still builds word by word instead of snapping whole.
const WORD_TAIL_SECS: f64 = 4.0;

/// The wheel delta one lyric-line step costs when scrolling the followed
/// sheet. A wheel notch arrives as three lines, so one notch steps to the
/// next sung line; a trackpad accumulates smoothly toward the same.
const SCROLL_STEP_LINES: f32 = 3.0;

/// How long a sung line holds before a woven gap rest takes over, in
/// seconds, capped at half the gap so a short gap still splits evenly. The
/// last words stay lit for a beat, then the sheet moves to the rest.
const REST_HOLD_SECS: f64 = 4.0;

/// The gap-threshold slider's range, in seconds: how long a gap or intro
/// must run before a rest is woven in.
const GAP_MIN: f32 = 1.0;
const GAP_MAX: f32 = 20.0;

/// Below this panel height the empty face stops stacking its "no lyrics"
/// line over the search button and flows them onto one row, so both still
/// show when the panel is short.
const EMPTY_INLINE_MAX_H: f32 = 120.0;

/// Only auto-save a searched sheet this confident or better, so an
/// automatic write never lands a loose guess on the track the way a manual
/// look would catch. Below it, the empty face waits for the manual search.
const AUTO_SAVE_CONFIDENCE: f32 = 0.9;

/// The line-spacing slider's range: the row-height multiplier over the text
/// size, from lines nearly touching to loosely spread.
const SPACING_MIN: f32 = 1.2;
const SPACING_MAX: f32 = 3.0;

/// A synced line's row height for a given text size and spacing multiplier:
/// enough lead that the karaoke lines breathe. Rows stay uniform so the
/// glide can center a line by index; the unsynced sheet wraps freely on its
/// own scroll instead.
fn line_height(font: f32, spacing: f32) -> f32 {
    font * spacing
}

/// Which side of the active line the falloff dims: the sung lines above,
/// the upcoming lines below, or both toward a center focus.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DimEdge {
    Top,
    Bottom,
    #[default]
    Both,
}

impl DimEdge {
    /// Whether a line at `distance` lines above (negative) or below
    /// (positive) the active one falls off on this edge.
    fn dims(self, above: bool) -> bool {
        match self {
            DimEdge::Top => above,
            DimEdge::Bottom => !above,
            DimEdge::Both => true,
        }
    }
}

/// What a wordless line shows in the synced sheet: a woven rest or a blank
/// line in the source. The note reads as a musical rest; none leaves the
/// row empty. A few picks rather than a free field, the panel-config idiom.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RestMark {
    #[default]
    Note,
    Dots,
    None,
}

impl RestMark {
    /// The glyph drawn on a wordless line.
    fn str(self) -> &'static str {
        match self {
            RestMark::Note => "\u{266a}",
            RestMark::Dots => "\u{2026}",
            RestMark::None => "",
        }
    }
}

/// The lyrics panel's per-view config: what a saved layout restores and
/// what the settings window edits. Missing fields take the defaults, so a
/// layout dumped before a knob existed still loads.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LyricsConfig {
    /// The rename, theme override, and placement locks shared by every
    /// panel.
    #[serde(flatten)]
    pub chrome: PanelChrome,
    pub source: TrackSource,
    pub align: Align,
    /// The lyric font family; None inherits the app font. A name that is
    /// not installed falls back to the default at render, so a layout moved
    /// between machines still shows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    /// Render the lyrics bold.
    pub bold: bool,
    /// The lyric text size in px, within [`FONT_MIN`]..[`FONT_MAX`]. The
    /// synced row height tracks it so bigger text keeps its lead.
    pub font_size: f32,
    /// The synced row height as a multiple of the text size, within
    /// [`SPACING_MIN`]..[`SPACING_MAX`]. Higher spreads the karaoke lines
    /// apart; the plain sheet wraps on its own and ignores this.
    pub line_spacing: f32,
    /// Glide the active line to the middle as playback moves through a
    /// synced sheet. Off leaves the list where the user scrolled it.
    pub follow: bool,
    /// Pad the synced list top and bottom so the first and last lines can
    /// glide to the middle too, keeping the active line always centered.
    pub pre_scroll: bool,
    /// Fade a synced line up from dim as it becomes the active one.
    pub fade_lines: bool,
    /// Build the active synced line word by word across its span, the rest
    /// of the words waiting dim until the playhead reaches them.
    pub word_by_word: bool,
    /// Weave a blank rest before a first sung line that opens past the
    /// [`gap_secs`] threshold, so the sheet has a lead-in to sit on and the
    /// first line fades in when it arrives.
    pub intro_rest: bool,
    /// Weave a blank rest into each instrumental gap between sung lines
    /// wider than [`gap_secs`], so the follow moves to a rest instead of
    /// holding the last line through the break.
    pub gap_rest: bool,
    /// How long a gap or intro must run, in seconds, before a rest is woven
    /// in. Governs both [`intro_rest`] and [`gap_rest`].
    pub gap_secs: f32,
    /// How much each line dims per step away from the active one, 0 to 1;
    /// 0 leaves every line at full. Applied on the [`dim_edge`] side.
    pub dim: f32,
    /// Which side of the active line the falloff dims.
    pub dim_edge: DimEdge,
    /// Show the "search online" button on the empty face while a lyrics
    /// provider is enabled. Off leaves the empty face just the quiet line,
    /// the right-click menu still reaching the search.
    pub search_button: bool,
    /// When a shown track carries no lyrics, search online in the
    /// background and save a confident match without opening the picker.
    /// Off leaves the empty face to the manual search.
    pub auto_search: bool,
    /// Show the shown track's name on the empty face, over the quiet "no
    /// lyrics" line, so a track with no words still says what it is.
    pub show_name: bool,
    /// Pin the track's title above an unsynced sheet, so a panel too short
    /// to show the words still reads as the song it belongs to.
    pub show_title: bool,
    /// What a wordless line shows in the synced sheet: a rest note, dots, or
    /// nothing.
    pub rest_mark: RestMark,
}

impl Default for LyricsConfig {
    fn default() -> Self {
        LyricsConfig {
            chrome: PanelChrome::default(),
            source: TrackSource::default(),
            // Lyrics read centered by default, the way a lyric sheet is
            // meant to; the align knob still moves them left or right.
            align: Align::Center,
            font: None,
            bold: false,
            font_size: 18.0,
            line_spacing: 1.9,
            follow: true,
            pre_scroll: true,
            fade_lines: false,
            word_by_word: false,
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

pub struct LyricsPanel {
    state: AppState,
    config: LyricsConfig,
    /// The loaded lyrics keyed by the track they belong to; None inside
    /// means that track carries none. Cleared on a library update or a
    /// save, so the next render re-reads.
    loaded: Option<(PathBuf, Option<Arc<Lyrics>>)>,
    /// The track a load is running for, so a render can tell "already
    /// fetching" from "needs a fetch".
    pending: Option<PathBuf>,
    /// Discards stale load results when the track changes mid-read.
    generation: u64,
    /// The cached source resolve, so the pump's per-frame notifies never
    /// turn into selection lookups.
    resolved: ResolvedTrack,
    /// The loaded sheet with the configured rests woven in, what the synced
    /// face actually walks. Keyed by the raw sheet's pointer and a rest-knob
    /// signature so it rebuilds only when the sheet or a knob changes.
    display: Option<((usize, u64), Arc<Lyrics>)>,
    /// The synced line under the playhead this render, for the highlight
    /// and the glide target. Indexes the woven [`display`] lines.
    active_line: Option<usize>,
    /// The line the current fade-in belongs to, so the fade resets when
    /// the active line moves on.
    faded_line: Option<usize>,
    /// The active line's fade-in progress, 0 to 1; 1 when fading is off.
    active_fade: f32,
    /// The active line's word-build fraction, 0 to 1; None when nothing is
    /// building.
    reveal: Option<f32>,
    /// The playhead sits on the shown track this render. Word-by-word only
    /// hides un-reached lines while this holds; a sheet viewed with no
    /// playhead on it still reads whole.
    positioned: bool,
    /// The blank rows padding each end of the synced list this frame, so
    /// line_rows maps a row index back to its lyric line.
    pad: usize,
    /// The line the follow glide is easing toward; None once arrived.
    glide_to: Option<usize>,
    /// Last frame's clock, for the glide's per-frame step.
    last_tick: Instant,
    /// The synced list's scroll, driven by the glide.
    scroll: UniformListScrollHandle,
    /// Wheel delta banked toward the next lyric-line step, so a slow scroll
    /// still lands one line at a time and the remainder carries over.
    scroll_accum: f32,
    /// The unsynced sheet's own scroll, so wrapped text scrolls freely.
    text_scroll: ScrollHandle,
    /// The text-size slider's drag state on the Appearance page.
    size_scrub: ScrubState,
    /// The line-spacing slider's drag state on the Appearance page.
    spacing_scrub: ScrubState,
    /// The line-falloff slider's drag state on the Content page.
    dim_scrub: ScrubState,
    /// The gap-threshold slider's drag state on the Content page.
    gap_scrub: ScrubState,
    /// The empty face's measured size, so it can flow its line and search
    /// button inline once the panel is too short to stack them.
    empty_size: Size<Pixels>,
    /// The track auto-search has already fired for, so it runs once per
    /// track no matter how many frames the empty face paints.
    auto_tried: Option<PathBuf>,
    focus: FocusHandle,
    tab_panel: Option<WeakEntity<TabPanel>>,
    _player_changed: Subscription,
    _selection_changed: Subscription,
    _library_changed: Subscription,
}

impl LyricsPanel {
    pub fn new(state: AppState, config: LyricsConfig, cx: &mut Context<Self>) -> Self {
        // The synced highlight follows the playhead, but only steps when the
        // lit line changes, so gate the pump's per-tick notify on that.
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
        // A rescan can rewrite tags and id -> path mappings; drop the
        // caches so the resolve and the lyrics re-read.
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.resolved.invalidate();
                this.loaded = None;
                cx.notify();
            },
        );
        LyricsPanel {
            state,
            config,
            loaded: None,
            pending: None,
            generation: 0,
            resolved: ResolvedTrack::default(),
            display: None,
            active_line: None,
            faded_line: None,
            active_fade: 1.0,
            reveal: None,
            positioned: false,
            pad: 0,
            glide_to: None,
            last_tick: Instant::now(),
            scroll: UniformListScrollHandle::new(),
            scroll_accum: 0.0,
            text_scroll: ScrollHandle::new(),
            size_scrub: ScrubState::default(),
            spacing_scrub: ScrubState::default(),
            dim_scrub: ScrubState::default(),
            gap_scrub: ScrubState::default(),
            empty_size: Size::default(),
            auto_tried: None,
            focus: cx.focus_handle(),
            tab_panel: None,
            _player_changed,
            _selection_changed,
            _library_changed,
        }
    }

    /// Make sure the lyrics for `path` are cached or on their way: read
    /// the file off the UI thread and swap the result in when done.
    fn ensure_loaded(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.loaded.as_ref().map(|(p, _)| p.as_path()) == Some(path)
            || self.pending.as_deref() == Some(path)
        {
            return;
        }
        self.pending = Some(path.to_path_buf());
        self.generation += 1;
        let generation = self.generation;
        let path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move { lyrics::load(&path, Some(&lyrics_dir())).map(Arc::new) }
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending = None;
                this.loaded = Some((path, loaded));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The lyrics loaded for `path`, or None while still loading or when
    /// the track carries none.
    fn lyrics_for(&self, path: &Path) -> Option<&Arc<Lyrics>> {
        self.loaded
            .as_ref()
            .filter(|(p, _)| p == path)
            .and_then(|(_, lyrics)| lyrics.as_ref())
    }

    /// The version of `raw` the synced face walks: the same sheet with the
    /// configured rests woven in. Cached by the raw sheet's identity and the
    /// rest knobs, so it rebuilds only when the sheet or a knob changes and
    /// every frame between reuses the woven lines. A reload hands a fresh
    /// pointer, so a re-read never reads through a stale weave.
    fn display_lyrics(&mut self, raw: &Arc<Lyrics>) -> Arc<Lyrics> {
        let key = (Arc::as_ptr(raw) as usize, self.rest_sig());
        if let Some((cached, lyrics)) = &self.display {
            if *cached == key {
                return lyrics.clone();
            }
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

    /// The woven lines cached this frame, what [`line_rows`] and the scroll
    /// step read after [`synced_face`] has built them.
    fn display_arc(&self) -> Option<&Arc<Lyrics>> {
        self.display.as_ref().map(|(_, lyrics)| lyrics)
    }

    /// A signature of the knobs that shape the weave, so the cache drops
    /// when any of them moves.
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

    /// The panel's own dropdown entries: the source pick and the follow
    /// toggle, the same knobs the customize window edits.
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
            PopupMenuItem::new("Follow Playback")
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

    /// Open the edit window on the shown track: it reads the file's words
    /// into a multi-line input, stamps lines against playback, and saves
    /// back where they came from, then pokes [`Self::reload`]. One window
    /// per track; a second request focuses the open one.
    fn open_edit(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.resolved.get(self.config.source, &self.state, cx) else {
            return;
        };
        crate::lyrics::edit::open(self.state.clone(), cx.entity().downgrade(), path, cx);
    }

    /// The timestamp `steps` sung lines away from the active one: forward
    /// for a positive step, back for a negative one, clamped to the ends.
    /// None when there is no loaded sheet, no timed lines, or the step
    /// would run off the top during the intro before the first line lights.
    fn walk_lines(&self, steps: i32) -> Option<f64> {
        // The woven sheet, so a step lands on the rests too.
        let lyrics = self.display_arc()?;
        // Only the timed lines can be seeked to; blanks and section marks
        // fall between them.
        let timed: Vec<f64> = lyrics.lines.iter().filter_map(|line| line.at).collect();
        if timed.is_empty() {
            return None;
        }
        // The active line's slot among the timed lines, or just before the
        // first (-1) during the intro when nothing is lit yet.
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

    /// Where playback sits within `path`, or None when a different track
    /// (or nothing) is playing. The stamp button and the synced highlight
    /// both key off this.
    fn playback_position(&self, path: &Path, cx: &App) -> Option<f64> {
        self.state
            .player
            .read(cx)
            .now_playing()
            .filter(|now| now.path == *path)
            .map(|now| now.position_secs)
    }

    /// Whether a pump tick is worth a repaint. Only a synced sheet under a
    /// live playhead is, and only when the track turns over or the lit line
    /// moves. The fade, the word-build, and the glide keep their own frames
    /// once a render runs, so the tick just wakes the panel that was parked
    /// between line changes instead of repainting it 60 times a second.
    fn tick_wakes(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(path) = self.resolved.get(self.config.source, &self.state, cx) else {
            // The source lost its track: repaint to the placeholder if a
            // sheet was up.
            return self.loaded.is_some() || self.pending.is_some();
        };
        // A different track needs a load and a fresh face; let the render
        // kick the fetch. Once it is loading, wait for the load's own notify.
        if self.loaded.as_ref().map(|(p, _)| p.as_path()) != Some(path.as_path()) {
            return self.pending.as_deref() != Some(path.as_path());
        }
        let Some(lyrics) = self.lyrics_for(&path).cloned() else {
            return false;
        };
        if !lyrics.synced {
            return false;
        }
        // Over the woven sheet, so the compared index matches the one the
        // render stores and the rests count as line changes worth a wake.
        let lyrics = self.display_lyrics(&lyrics);
        let active = self
            .playback_position(&path, cx)
            .and_then(|secs| active_line(&lyrics, secs));
        active != self.active_line
    }

    /// Open the match window on the shown track: it searches online,
    /// ranks candidates by confidence, and saves the one the user
    /// confirms, so nothing is written before a look. The window pokes
    /// [`Self::reload`] on save.
    fn open_match(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.resolved.get(self.config.source, &self.state, cx) else {
            return;
        };
        crate::lyrics::matcher::open(self.state.clone(), cx.entity().downgrade(), path, cx);
    }

    /// Drop the cached sheet for `path` and repaint, so a save made
    /// outside the panel (the match window) shows on the next render.
    /// Lyrics do not ride the projection, so this is the panel's only
    /// signal to re-read.
    pub fn reload(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.loaded.as_ref().is_some_and(|(p, _)| p == path) {
            self.loaded = None;
        }
        cx.notify();
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
                "Follow Playback",
                Some("Glide the active line to the middle as a synced sheet plays"),
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
                "Always Centered",
                Some("Pad the ends so the first and last lines can center too"),
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
                "Fade Lines In",
                Some("Fade a line up from dim as it becomes the active one"),
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
                "Build Word by Word",
                Some("Reveal words as they are sung, karaoke style; unsung lines wait hidden"),
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
                "Lead-in Rest",
                Some("Sit on a blank rest before a long intro, so the first line fades in when it arrives"),
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
                "Rest in Gaps",
                Some("Move to a blank rest through a long instrumental gap instead of holding the last line"),
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
                let fraction =
                    ((self.config.gap_secs - GAP_MIN) / (GAP_MAX - GAP_MIN)).clamp(0., 1.);
                d.child(panel::setting_row(
                    "Gap Threshold",
                    Some("How long an intro or gap must run to earn a rest"),
                    settings_ui::slider_labeled(
                        &self.gap_scrub,
                        fraction,
                        format!("{}s", self.config.gap_secs.round() as u32),
                        |this: &mut Self, fraction, cx| {
                            this.config.gap_secs = GAP_MIN + fraction * (GAP_MAX - GAP_MIN);
                            cx.notify();
                        },
                        cx,
                    ),
                ))
            })
            .child(panel::setting_row(
                "Line Falloff",
                Some("How far each line dims per step away from the active one"),
                settings_ui::slider_labeled(
                    &self.dim_scrub,
                    self.config.dim,
                    format!("{}%", (self.config.dim * 100.0).round() as u32),
                    |this: &mut Self, fraction, cx| {
                        this.config.dim = fraction;
                        cx.notify();
                    },
                    cx,
                ),
            ))
            .child(panel::setting_row(
                "Falloff Edge",
                Some("Which side of the active line the falloff dims"),
                panel::choices(
                    &[
                        ("Top", DimEdge::Top),
                        ("Bottom", DimEdge::Bottom),
                        ("Both", DimEdge::Both),
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
                "Online Search Button",
                Some(
                    "Show the search button on the empty face; the right-click menu still finds lyrics",
                ),
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
                "Auto Search",
                Some("Search online on a track with no words and save a confident match, no picker"),
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
                "Show Song Name",
                Some("Show the track's name on the empty face, over the no-lyrics line"),
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
                "Title on Unsynced",
                Some("Pin the track's title above an unsynced sheet, so a short panel still shows it"),
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
                "Rest Marker",
                Some("What a wordless line shows in a synced sheet, the gaps and blank lines"),
                panel::choices(
                    &[
                        ("Note", RestMark::Note),
                        ("Dots", RestMark::Dots),
                        ("None", RestMark::None),
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

    // The lyric appearance section carries its own font picker beside the
    // weight and size knobs, so the shared page leaves off its generic one.
    fn has_own_font(&self) -> bool {
        true
    }

    /// The lyric type controls live on the Appearance page beside the
    /// shared frame and color knobs, the grid's tile-size move: the font
    /// family, weight, and size.
    fn appearance(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let fraction = ((self.config.font_size - FONT_MIN) / (FONT_MAX - FONT_MIN)).clamp(0., 1.);
        let spacing_fraction = ((self.config.line_spacing - SPACING_MIN)
            / (SPACING_MAX - SPACING_MIN))
            .clamp(0., 1.);
        // Reset the lyric type back to its defaults: family off to the app
        // font, weight, size, and spacing to the built-in look.
        let reset = settings_ui::small_button(
            "Reset",
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
                "Lyrics",
                Some(reset.into_any_element()),
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .child(panel::setting_row(
                        "Font",
                        Some("The lyric typeface; default follows the app font"),
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
                        "Bold",
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
                        "Text Size",
                        Some("The lyric text; the synced line height follows it"),
                        settings_ui::slider_labeled(
                            &self.size_scrub,
                            fraction,
                            format!("{}px", self.config.font_size.round() as u32),
                            |this: &mut Self, fraction, cx| {
                                this.config.font_size = FONT_MIN + fraction * (FONT_MAX - FONT_MIN);
                                cx.notify();
                            },
                            cx,
                        ),
                    ))
                    .child(panel::setting_row(
                        "Line Spacing",
                        Some("How far the synced lines sit apart, as a multiple of the text size"),
                        settings_ui::slider_labeled(
                            &self.spacing_scrub,
                            spacing_fraction,
                            format!("{:.1}x", self.config.line_spacing),
                            |this: &mut Self, fraction, cx| {
                                this.config.line_spacing =
                                    SPACING_MIN + fraction * (SPACING_MAX - SPACING_MIN);
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

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        panel::title_text(self.config.chrome.title.as_deref(), "Lyrics")
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        self.config.chrome.title.clone().map(SharedString::from)
    }

    /// The edit pencil shares the title bar row, the metadata panel's move.
    /// It opens the edit window; hidden while the panel shows no track.
    fn title_suffix(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        self.resolved.get(self.config.source, &self.state, cx)?;
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
        // Opens the edit window, the same one the title-bar pencil drives,
        // so a right click reaches it too.
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new("Edit Lyrics")
                .icon(Icon::default().path(icons::PENCIL))
                .on_click(move |_, _, cx| {
                    let Some(this) = weak.upgrade() else { return };
                    this.update(cx, |this, cx| this.open_edit(cx));
                }),
        );
        // The online search, gated with the provider toggle so the menu
        // never offers a lookup that can't run. Opens the match window;
        // the write waits for a confirmed pick.
        let menu = if providers::lyrics_online() {
            let weak = cx.entity().downgrade();
            menu.item(
                PopupMenuItem::new("Find Lyrics Online...")
                    .icon(Icon::default().path(icons::DOWNLOAD))
                    .on_click(move |_, _, cx| {
                        let Some(this) = weak.upgrade() else { return };
                        this.update(cx, |this, cx| this.open_match(cx));
                    }),
            )
        } else {
            menu
        };
        let menu = panel_settings::rename_item(menu, &cx.entity(), self.tab_panel.clone(), window, cx);
        let menu = panel_settings::settings_item(menu, &cx.entity());
        let menu = panel::duplicate_item(menu, &cx.entity(), self.tab_panel.clone(), |this, _window, cx| {
            let (state, config) = {
                let panel = this.read(cx);
                (panel.state.clone(), panel.config.clone())
            };
            LyricsPanel::new(state, config, cx)
        });
        panel::popout_item(
            menu,
            &cx.entity(),
            self.tab_panel.clone(),
            self.state.clone(),
        )
    }
}

impl Render for LyricsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chrome = self.config.chrome.clone();
        panel::themed(&chrome, || self.body(window, cx))
    }
}

impl LyricsPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // Solo there is no tab bar to host the edit pencil, but a body
        // toolbar just to carry it eats space that reads as chrome; the
        // right-click menu's Edit Lyrics opens the edit window instead.
        // Tabbed, the pencil rides the tab bar through the title suffix.
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette::bg_root())
            .when_some(self.config.font.clone(), |d, font| d.font_family(font))
            .when(self.config.bold, |d| d.font_weight(FontWeight::BOLD))
            .child(self.content(window, cx).flex_1().min_h_0())
    }

    /// The panel body: the display face, a synced karaoke list, a plain
    /// sheet, or a quiet placeholder. Editing lives in its own window.
    fn content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(path) = self.resolved.get(self.config.source, &self.state, cx) else {
            // With the name showing, a track stands in for the panel's face,
            // so with none the panel reads as empty rather than flashing a
            // "No track" notice.
            return if self.config.show_name {
                div().size_full()
            } else {
                placeholder("No track")
            };
        };

        self.ensure_loaded(&path, cx);
        let Some(lyrics) = self.lyrics_for(&path).cloned() else {
            // Still loading, or the track carries none.
            return if self.pending.as_deref() == Some(path.as_path()) {
                loading()
            } else {
                self.empty_face(&path, cx)
            };
        };

        if lyrics.synced {
            self.synced_face(&path, &lyrics, window, cx)
        } else {
            self.plain_face(&path, &lyrics, cx)
        }
    }

    /// The empty face: the quiet "no lyrics" line, with the online search
    /// beside or under it while a lyrics provider is enabled and the button
    /// is not hidden. The search opens the match window rather than writing
    /// straight away. The whole face honors the panel's alignment, and once
    /// the panel is too short to stack the line over the button it flows
    /// them onto one row so both still show. Auto-search kicks off here too.
    fn empty_face(&mut self, path: &Path, cx: &mut Context<Self>) -> Div {
        self.maybe_auto_search(path, cx);
        let align = self.config.align;
        // Unmeasured (height 0) stacks; only a measured, short panel flows
        // the line and button inline, so the first frame never flickers.
        let inline =
            self.empty_size.height > px(0.) && self.empty_size.height < px(EMPTY_INLINE_MAX_H);
        let show_button = self.config.search_button && providers::lyrics_online();
        let button = show_button.then(|| {
            settings_ui::small_button(
                "Search Online",
                icons::DOWNLOAD,
                false,
                cx.listener(|this, _, _, cx| this.open_match(cx)),
            )
        });
        // The track's name over the quiet line, so a wordless track still
        // says what it is. Title falls back to the file stem, the artist
        // trailing it when the tags carry one.
        let name = self
            .config
            .show_name
            .then(|| self.track_name(path, cx))
            .filter(|name| !name.is_empty());

        let face = div()
            .size_full()
            .relative()
            .flex()
            .gap(tokens::SPACE_SM)
            .p(tokens::SPACE_MD);
        // A row aligns along its main axis, a column along the cross axis,
        // so the same alignment knob reads the same either way.
        let face = if inline {
            justify(face.flex_row().items_center(), align)
        } else {
            items(face.flex_col().justify_center(), align)
        };
        // With the name showing, it stands in for the quiet line, so a
        // wordless track reads as itself rather than a "no lyrics" notice.
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
            d.child(div().text_color(palette::text_faint()).child("No lyrics"))
        })
        .when_some(button, |d, button| d.child(button))
        // A zero-layout canvas over the face reports its size so the next
        // frame can pick the stacked or inline shape.
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

    /// The shown track's name for the empty face: its title, then the
    /// artist when the tags carry one, the file stem standing in for a
    /// missing title.
    fn track_name(&self, path: &Path, cx: &App) -> SharedString {
        let meta = self.state.library.read(cx).meta_for(path);
        let (title, artist) = meta.map(|m| (m.title, m.artist)).unwrap_or_default();
        let title = if title.is_empty() {
            path.file_stem()
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

    /// With auto-search on, look the shown track up online in the
    /// background the first time its empty face paints, and save the top
    /// match when it clears [`AUTO_SAVE_CONFIDENCE`]. A weak match is left
    /// alone for the manual search, which shows every candidate. Runs once
    /// per track path so a repaint never re-queries.
    fn maybe_auto_search(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.config.auto_search || !providers::lyrics_online() {
            return;
        }
        if self.auto_tried.as_deref() == Some(path) {
            return;
        }
        self.auto_tried = Some(path.to_path_buf());
        let query = crate::lyrics::matcher::query_for(&self.state, path, cx);
        if query.artist.is_empty() || query.title.is_empty() {
            return;
        }
        let path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move {
                        let found = providers::search_lyrics(&query).ok()?;
                        let best = found.into_iter().next()?;
                        if best.confidence < AUTO_SAVE_CONFIDENCE {
                            return None;
                        }
                        let target = crate::lyrics::matcher::save_target(&path);
                        lyrics::save(&path, &target, &best.text).ok()?;
                        Some(())
                    }
                })
                .await;
            if saved.is_some() {
                this.update(cx, |this, cx| this.reload(&path, cx)).ok();
            }
        })
        .detach();
    }

    /// The synced face: one row per timed line, the line under the
    /// playhead lit and gliding to the middle while follow is on, with the
    /// optional fade-in and word-build effects layered on. Clicking a line
    /// seeks to it. Blank pad rows at the ends let the first and last lines
    /// reach the middle too when pre-scroll is on.
    fn synced_face(
        &mut self,
        path: &Path,
        lyrics: &Arc<Lyrics>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        // Walk the woven sheet, not the raw one: the rests are real timed
        // lines to the follow, the highlight, and the scroll step.
        let lyrics = self.display_lyrics(lyrics);
        // The playhead only speaks for the track it is on; a Selected
        // source pointed elsewhere reads no position and just scrolls.
        let position = self.playback_position(path, cx);
        let active = position.and_then(|secs| active_line(&lyrics, secs));
        self.active_line = active;
        self.positioned = position.is_some();

        let dt = self.last_tick.elapsed().as_secs_f32().min(0.05);
        self.last_tick = Instant::now();
        let mut animating = false;

        // Fade-in: reset a line to the floor when it takes over, easing it
        // up to full over FADE_SECS. Off keeps every line at full.
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

        // Word-build: how far the playhead has run into the active line's
        // span, as a fraction, so line_rows lights words up to there.
        self.reveal = None;
        if self.config.word_by_word {
            if let (Some(pos), Some(ix)) = (position, active) {
                let start = lyrics.lines[ix].at.unwrap_or(pos);
                let end = lyrics.lines[ix + 1..]
                    .iter()
                    .find_map(|line| line.at)
                    .unwrap_or(start + WORD_TAIL_SECS);
                let frac = if end > start {
                    ((pos - start) / (end - start)) as f32
                } else {
                    1.0
                };
                self.reveal = Some(frac.clamp(0.0, 1.0));
            }
        }
        // The word-build tracks the playhead across the line, so keep the
        // frames coming while it still has words to light; the pump's tick
        // no longer wakes the panel between line changes.
        animating |= self.reveal.is_some_and(|frac| frac < 1.0);

        // Pad the ends so the first and last lines can center as well.
        let pad = self.pad_rows(line_height(self.config.font_size, self.config.line_spacing));
        self.pad = pad;
        let count = pad + lyrics.lines.len() + pad;

        // Re-aim the glide when the active line moves; drive it toward the
        // middle here in render, the grid's follow idiom, asking for the
        // next frame only while it still moves.
        if self.config.follow {
            if let Some(active) = active {
                self.glide_to = Some(active + pad);
            }
        }
        if let Some(row) = self.glide_to {
            let arrived = match panel::glide_target(&self.scroll, row, count) {
                Some(target) => !panel::glide_step(&self.scroll, target, dt),
                None => false,
            };
            if arrived {
                self.glide_to = None;
            } else {
                animating = true;
            }
        }
        if animating {
            window.request_animation_frame();
        }

        let this = cx.entity().downgrade();
        div()
            .size_full()
            // With follow on, the glide pins the sheet to the playhead so the
            // list never free-scrolls anyway; repurpose the wheel there to
            // step through the sung lines, seeking the song to each and
            // letting the follow glide the sheet onto it. With follow off, or
            // no playhead on this track, the wheel scrolls to read as usual.
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
                // Bank the delta and spend it a line at a time: wheel down
                // (content up, toward later lines) steps forward, up steps
                // back, the same direction the follow scrolls as it plays.
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
                uniform_list("lyrics-lines", count, move |range, _, cx| {
                    this.upgrade()
                        .map(|this| this.update(cx, |this, cx| this.line_rows(range, cx)))
                        .unwrap_or_default()
                })
                .track_scroll(self.scroll.clone())
                .size_full(),
            )
    }

    /// The blank rows to pad each end of the synced list with, so the
    /// first and last lines can glide to the middle. Half the viewport in
    /// rows when pre-scroll is on, off the list's measured height (a
    /// generous default before the first layout); none when it is off.
    fn pad_rows(&self, line_h: f32) -> usize {
        if !self.config.pre_scroll {
            return 0;
        }
        let viewport = self
            .scroll
            .0
            .borrow()
            .last_item_size
            .map(|size| f32::from(size.item.height))
            .filter(|h| *h > 0.0);
        match viewport {
            Some(h) => (((h / 2.0) / line_h).ceil() as usize).max(1),
            None => 12,
        }
    }

    /// The rows a synced list asks for: blank pad rows at the ends, and
    /// between them each timed line at a height that tracks the text size,
    /// the active one lit (faded in and built word by word when those are
    /// on), the rest muted, all clickable to seek. Full width, so the
    /// alignment knob actually centers the text.
    fn line_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        // The woven sheet synced_face built this frame, rests and all.
        let Some(lyrics) = self.display_arc().cloned() else {
            return Vec::new();
        };
        let active = self.active_line;
        let align = self.config.align;
        let font = self.config.font_size;
        let line_h = line_height(font, self.config.line_spacing);
        let pad = self.pad;
        let fade = self.active_fade;
        let fade_lines = self.config.fade_lines;
        let reveal = self.reveal;
        let word_by_word = self.config.word_by_word;
        let positioned = self.positioned;
        let dim = self.config.dim;
        let dim_edge = self.config.dim_edge;
        let rest_mark = self.config.rest_mark.str();
        // Before the first line lights up, still measure the falloff from
        // where the read head is headed (the first timed line) so a live
        // intro shows the sheet already dimmed toward the edge instead of
        // sitting flat until the first word passes. With no playhead on the
        // track there is nothing to anchor to, so the sheet reads whole.
        let falloff_from = active.or_else(|| {
            positioned.then(|| {
                lyrics
                    .lines
                    .iter()
                    .position(|line| line.at.is_some())
                    .unwrap_or(0)
            })
        });
        range
            .map(|row_ix| {
                // The pad rows at the ends are blank spacers.
                let Some(ix) = row_ix
                    .checked_sub(pad)
                    .filter(|&ix| ix < lyrics.lines.len())
                else {
                    return div().h(px(line_h)).into_any_element();
                };
                let line = &lyrics.lines[ix];
                let at = line.at;
                let is_active = Some(ix) == active;
                let text = &line.text;

                // The active line builds word by word: each word waits
                // hidden, then fades and rises into place as the read head
                // reaches it. No reveal (paused off a position) shows the
                // whole line.
                let content: AnyElement = if is_active && word_by_word && !text.is_empty() {
                    let words: Vec<&str> = text.split_whitespace().collect();
                    let n = words.len().max(1);
                    let read_head = reveal.map(|frac| frac * n as f32);
                    let rise = font * WORD_RISE;
                    div()
                        .max_w_full()
                        .flex()
                        .flex_row()
                        .children(words.into_iter().enumerate().map(|(i, word)| {
                            let opacity = read_head
                                .map(|head| ((head - i as f32) / WORD_FADE).clamp(0.0, 1.0))
                                .unwrap_or(1.0);
                            div()
                                .relative()
                                .top(px((1.0 - opacity) * rise))
                                .opacity(opacity)
                                .child(SharedString::from(format!("{word}\u{a0}")))
                        }))
                        .into_any_element()
                } else {
                    div()
                        .max_w_full()
                        .truncate()
                        .child(if text.is_empty() {
                            SharedString::from(rest_mark)
                        } else {
                            SharedString::from(text.clone())
                        })
                        .into_any_element()
                };

                // Word-by-word keeps un-sung text out of sight: while the
                // playhead sits on this track, every line past the active
                // one (and every line during the intro) waits invisible
                // until its turn, its row still holding the space. With no
                // playhead on the track the sheet reads whole.
                let upcoming =
                    word_by_word && positioned && active.is_none_or(|active| ix > active);

                // The active line fades up from the floor; the others dim
                // by their distance from it, on the chosen edge.
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
                        .h(px(line_h))
                        .w_full()
                        .flex()
                        .flex_none()
                        .items_center()
                        .overflow_hidden(),
                    align,
                )
                .px(tokens::SPACE_MD)
                .text_size(px(font))
                .opacity(opacity)
                .text_color(if is_active {
                    palette::text_bright()
                } else {
                    palette::text_muted()
                })
                .child(content);
                // A timed line seeks to its own time on click.
                let row = row.when_some(at, |d, at| {
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
            .collect()
    }

    /// The plain face: the whole sheet as wrapped text on its own scroll,
    /// no highlight, no follow, for lyrics with no timestamps. With the
    /// title option on, the track name pins above the scroll so a short
    /// panel still says what song it is.
    fn plain_face(&self, path: &Path, lyrics: &Arc<Lyrics>, cx: &App) -> Div {
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
            // The spacing knob rides the text line height here, so it moves
            // an unsynced sheet the way it moves the synced rows.
            .line_height(px(font * spacing))
            .text_color(palette::text())
            .children(lyrics.lines.iter().map(|line| {
                // Blank source lines keep a gap that scales with the spacing.
                if line.text.is_empty() {
                    div().h(px(font * spacing * 0.5))
                } else {
                    div()
                        .w_full()
                        .text_align(text_align)
                        .child(SharedString::from(line.text.clone()))
                }
            }));
        // The title pins above the scroll as a fixed row, so it holds while
        // the sheet scrolls and stays put when a short panel squeezes the
        // words out.
        let title = self.config.show_title.then(|| self.track_name(path, cx));
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
                    // min_h_full plus a centering column centers a short
                    // sheet in the panel while a long one still scrolls from
                    // the top, the free-space-only trick.
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

/// A non-active line's opacity under the distance falloff: `dim` shaved
/// off per step away from the active line, compounding, on the chosen
/// edge. Full when there is no active line to measure from, when the
/// factor is zero, or when this line sits on the edge that does not dim.
fn falloff(dim: f32, edge: DimEdge, active: Option<usize>, ix: usize) -> f32 {
    let Some(active) = active.filter(|_| dim > 0.0) else {
        return 1.0;
    };
    let above = ix < active;
    if !edge.dims(above) {
        return 1.0;
    }
    let distance = ix.abs_diff(active) as i32;
    (1.0 - dim).powi(distance)
}

/// The loaded sheet with rests woven in: a leading blank ♪ before a first
/// sung line that opens past `gap_secs`, and a blank ♪ in each gap between
/// sung lines wider than `gap_secs`, placed a short hold after the line it
/// follows so the last words linger before the sheet moves to the rest.
/// The sheet comes back untouched when it carries no timing or both rests
/// are off.
fn weave_rests(raw: &Arc<Lyrics>, intro: bool, gap: bool, gap_secs: f64) -> Arc<Lyrics> {
    if !raw.synced || (!intro && !gap) {
        return raw.clone();
    }
    let mut lines = Vec::with_capacity(raw.lines.len() + 4);
    let mut prev_timed: Option<f64> = None;
    for line in &raw.lines {
        if let Some(at) = line.at {
            match prev_timed {
                // Before the first sung line: a lead-in rest when the intro
                // runs long enough to earn one.
                None if intro && at > gap_secs => lines.push(rest_line(0.0)),
                // Between two sung lines: a rest a short hold past the first,
                // clamped to before the midpoint so a shorter gap still
                // splits cleanly.
                Some(prev) if gap && at - prev > gap_secs => {
                    let hold = ((at - prev) * 0.5).min(REST_HOLD_SECS);
                    lines.push(rest_line(prev + hold));
                }
                _ => {}
            }
            prev_timed = Some(at);
        }
        lines.push(line.clone());
    }
    Arc::new(Lyrics {
        source: raw.source.clone(),
        text: raw.text.clone(),
        lines,
        synced: raw.synced,
    })
}

/// A blank timed line, shown as a ♪ rest and seekable like any other.
fn rest_line(at: f64) -> Line {
    Line {
        at: Some(at),
        text: String::new(),
    }
}

/// The last timed line at or before `position`, the one under the
/// playhead. None before the first line's time, so nothing lights up
/// during an intro.
fn active_line(lyrics: &Lyrics, position: f64) -> Option<usize> {
    let mut active = None;
    for (ix, line) in lyrics.lines.iter().enumerate() {
        match line.at {
            Some(at) if at <= position + 0.05 => active = Some(ix),
            Some(_) => break,
            None => {}
        }
    }
    active
}

/// A quiet centered line where the sheet would sit.
fn placeholder(text: &'static str) -> Div {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p(tokens::SPACE_MD)
        .text_color(palette::text_faint())
        .child(text)
}

/// A centered spinner while the sheet loads, so the wait reads as work in
/// progress rather than an empty panel.
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
