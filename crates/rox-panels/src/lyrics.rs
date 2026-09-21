//! The lyrics panel: the current track's words, timed against playback
//! when the file has an LRC-style sheet, plain scrolling text when it
//! doesn't. Which track is per-view config through [`TrackSource`], the
//! same knob the cover and metadata panels have, so a duplicate can watch
//! each. A synced sheet highlights the line under the playhead and, with
//! follow on, glides it to the middle the way the library's now-playing
//! row does; clicking a timed line seeks to it.
//!
//! With the read head on, the active line is cut where the playhead has
//! sung to and the unsung tail colored back, which is the karaoke fill. An
//! enhanced (A2) sheet times each word and drives that cut off its own
//! clock, so the head lands mid-word where the singer is; a plain
//! line-synced sheet has nothing finer to go on and spreads the text
//! across the line's span instead.
//!
//! The synced sheet builds every row rather than a window of them. Rows
//! wrap to as many visual lines as the words need, so there's no uniform
//! stride to virtualize against, and a sheet is a few dozen lines either
//! way. The scroll records what it laid out and the follow centers the
//! active line off those measured bounds.
//!
//! The pencil in the title row opens the edit window:
//! the raw text becomes a multi-line input over a baseline read off the
//! file, and a save writes it back where it came from: the embedded tag
//! through the writer's atomic layer, or the `.lrc` sidecar or app lyrics
//! store as a plain file. Lyrics aren't in the library projection, so a
//! save just re-reads the file. While that window is open it hands its
//! unsaved draft back here on every keystroke, so nudging a sheet's offset
//! moves the words in the panel as the arrow is pressed.
//!
//! Not every track is a file, and the two that aren't still get all of
//! this. What a sheet is filed under is a [`Subject`] rather than a path:
//! a Subsonic song under the id its server keeps handing back, and a radio
//! station's song under the artist and title it announced in band, since
//! the station's own row names the station for the whole broadcast.
//!
//! A station's words are timed against the song and not the listen, and
//! only when we heard the song begin. Tuning in lands in the middle of
//! whatever is on and the announcement that names it says nothing about
//! how far in, so that first song reads as a plain unsynced sheet however
//! the provider timed it. From the next turnover on the clock is real and
//! the sheet follows.

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

// The stamp action moved down with the rest of the panel seam; the editor
// window up here binds and handles it at the path it always did.
pub use rox_panel_api::actions::StampLine;

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
/// head gets to it, as a fraction of that slot. Smaller is snappier.
const WORD_FADE: f32 = 0.5;

/// How far a word rises into place as it fades in, as a fraction of the
/// text size.
const WORD_RISE: f32 = 0.35;

/// The wheel delta one lyric-line step costs when scrolling the followed
/// sheet. A wheel notch arrives as three lines, so one notch steps to the
/// next sung line; a trackpad accumulates smoothly toward the same.
const SCROLL_STEP_LINES: f32 = 3.0;

/// The gap-threshold slider's range, in seconds: how long a gap or intro
/// must run before a rest is woven in.
const GAP_MIN: f32 = 1.0;
const GAP_MAX: f32 = 20.0;

/// Below this panel height the empty face stops stacking its "no lyrics"
/// line over the search button and flows them onto one row, so both still
/// show when the panel is short.
const EMPTY_INLINE_MAX_H: f32 = 120.0;

/// Only auto-save a searched sheet this confident or better, so an
/// automatic write never puts a loose guess on the track the way a manual
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

/// How the active line shows the read head running through it.
///
/// Fill is the karaoke look: the whole line stays up and a brightness
/// boundary sweeps across it, landing mid-word on a sheet that times its
/// words. Build is the older one, each word waiting out of sight and
/// fading up as its turn comes.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WordStyle {
    #[default]
    Fill,
    Build,
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
    /// Run a read head through the active synced line as it is sung. An
    /// enhanced (A2) sheet drives it off its own word clock; a
    /// line-synced one spreads the text evenly across the line's span.
    pub word_by_word: bool,
    /// How that read head shows: the karaoke fill or the older per-word
    /// build.
    pub word_style: WordStyle,
    /// How dim the unsung half of the active line sits under the fill, 0
    /// to 1, where 1 is as dim as a line the playhead has left behind and
    /// 0 leaves it at full.
    pub word_dim: f32,
    /// Hide every line the playhead hasn't reached, so the sheet reveals
    /// itself as it is sung. Off shows the whole sheet with the falloff
    /// dimming it, which is how a lyric sheet normally reads.
    pub hide_upcoming: bool,
    /// Wrap a synced line too long for the panel onto as many rows as it
    /// needs. Off keeps every row one line tall and truncates.
    pub wrap_lines: bool,
    /// Weave a blank rest before a first sung line that opens past the
    /// [`gap_secs`] threshold, so the sheet has a lead-in and the
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
    /// When a shown track has no lyrics, search online in the
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

/// The lyrics target with what it was built from beside it. Building one
/// resolves the catalog and the render asks every frame, so the answer is
/// kept until the thing it was built from moves: the shown track, or the
/// song a station is announcing. A library update drops it too, which is
/// when the tags underneath could have changed.
struct TargetCache {
    key: TrackKey,
    /// The station-title revision the song was taken at, and what tells a
    /// held answer from a stale one without taking the title lock.
    rev: u64,
    /// None for a station between announcements, which has no song under
    /// it to find words for.
    built: Option<LyricsTarget>,
}

pub struct LyricsPanel {
    state: AppState,
    config: LyricsConfig,
    /// The loaded lyrics keyed by the subject they belong to; None inside
    /// means that subject has none. The flag is its "no lyrics" mark, read
    /// with the sheet so the empty face can tell a marked track from one
    /// nothing was ever found for without a stat per frame. Cleared on a
    /// library update or a save, so the next render re-reads.
    ///
    /// The subject is what keeps a station honest. A stream holds one URL
    /// for hours and turns its song over underneath, so a key built off
    /// the track would pin the first song's words up for the rest of the
    /// broadcast; the announced song is part of the subject, so the next
    /// song is a different key and reads as the miss it is.
    loaded: Option<(Subject, Option<Arc<Lyrics>>, bool)>,
    /// The subject a load is running for, so a render can tell "already
    /// fetching" from "needs a fetch".
    pending: Option<Subject>,
    /// The edit window's unsaved draft, shown in place of whatever is
    /// stored for as long as that window is open. This is what puts an
    /// offset nudge on screen the moment the arrow is pressed instead of
    /// at the save.
    preview: Option<(Subject, Arc<Lyrics>)>,
    /// The cached lyrics target, so a render never re-resolves the
    /// catalog.
    target: Option<TargetCache>,
    /// Discards stale load results when the track changes mid-read.
    generation: u64,
    /// The cached source resolve, so the pump's per-frame notifies never
    /// turn into selection lookups.
    resolved: ResolvedTrack,
    /// The loaded sheet with the configured rests woven in, what the synced
    /// face actually steps through. Keyed by the raw sheet's pointer and a rest-knob
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
    /// Where the read head sits in the active line's text this render, as
    /// a byte offset into it; None when the head isn't running.
    head: Option<usize>,
    /// The playhead is on the shown track this render. Word-by-word only
    /// hides un-reached lines while this holds; a sheet viewed with no
    /// playhead on it still reads whole.
    positioned: bool,
    /// The pad each end of the synced list carries this frame, so the
    /// first and last lines can still reach the middle.
    pad: Pixels,
    /// The synced sheet's own scroll once rows wrap and stop being a
    /// uniform height, so the glide can center a row off its real bounds.
    wrap_scroll: ScrollHandle,
    /// The line the follow glide is easing toward; None once arrived.
    glide_to: Option<usize>,
    /// Last frame's clock, for the glide's per-frame step.
    last_tick: Instant,
    /// Wheel delta banked toward the next lyric-line step, so a slow scroll
    /// still steps one line at a time and the remainder is kept.
    scroll_accum: f32,
    /// The unsynced sheet's own scroll, so wrapped text scrolls freely.
    text_scroll: ScrollHandle,
    /// The text-size slider's drag state on the Appearance page.
    size_scrub: ScrubState,
    /// The line-spacing slider's drag state on the Appearance page.
    spacing_scrub: ScrubState,
    /// The unsung-dim slider's drag state on the Content page.
    word_dim_scrub: ScrubState,
    /// The line-falloff slider's drag state on the Content page.
    dim_scrub: ScrubState,
    /// The gap-threshold slider's drag state on the Content page.
    gap_scrub: ScrubState,
    /// The one readout being typed into across the settings sliders.
    value_edit: panel::ValueEdit,
    /// The empty face's measured size, so it can flow its line and search
    /// button inline once the panel is too short to stack them.
    empty_size: Size<Pixels>,
    /// The subject auto-search has already fired for, so it runs once per
    /// track no matter how many frames the empty face paints. A subject
    /// rather than a track for the same reason the sheet cache is one: a
    /// station would otherwise look its first song up and then sit there
    /// wordless for every song after it.
    auto_tried: Option<Subject>,
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
                this.target = None;
                cx.notify();
            },
        );
        // A sheet saved anywhere refreshes every panel, not just the one whose
        // pencil opened the window.
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

    /// The station-title revision the shown track sits at: the number that
    /// moves when a stream announces its next song. Zero unless the shown
    /// track is the live one playing, so a file and a selection both key
    /// on nothing but themselves.
    fn live_rev(&self, key: &TrackKey, cx: &App) -> u64 {
        let player = self.state.player.read(cx);
        match player.now_playing() {
            Some(now) if now.live && now.key == *key => player.title_rev().unwrap_or(0),
            _ => 0,
        }
    }

    /// What the panel files and looks a sheet up under, with the provider
    /// query beside it. Cached against the track and the announced song,
    /// since building one resolves the catalog and the render asks every
    /// frame.
    ///
    /// None for a station that hasn't named a song yet, the one track with
    /// nothing to go on: its row says what the station is called and there
    /// is no song under it to find words for.
    fn target(&mut self, key: &TrackKey, cx: &App) -> Option<&LyricsTarget> {
        // The revision is an atomic; the title behind it is a lock and a
        // pair of string clones, so only a frame where a station actually
        // moved on goes and takes one. That is what the player publishes a
        // revision for.
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

    /// Whether a timed sheet can be followed against this track. A file
    /// plays from its own zero, so always.
    ///
    /// A station only once we heard the song begin. Tuning in lands in the
    /// middle of whatever is on and the announcement naming it says
    /// nothing about how far in, so following the stamps would light lines
    /// a minute or two off the words. Those same words still read fine as
    /// an unsynced sheet, which is where the plain face takes them.
    fn can_sync(&self, key: &TrackKey, cx: &App) -> bool {
        match self.state.player.read(cx).now_playing() {
            Some(now) if now.live && now.key == *key => now.song_from_start,
            // Nothing timing against this track, so there is no clock here
            // to be wrong about.
            _ => true,
        }
    }

    /// The shown track's tags with a station's announced song laid over
    /// them. A stream's library row names the station and never moves, so
    /// anything that reads a title or an artist off the row has to come
    /// through here or it reads the station's name where the song belongs.
    fn live_meta(&self, key: &TrackKey, cx: &App) -> Option<rox_library::store::TrackMeta> {
        let row = self.state.library.read(cx).meta_for_key(key);
        let player = self.state.player.read(cx);
        match player.now_playing() {
            Some(now) if now.key == *key => player.live_over(row),
            _ => row,
        }
    }

    /// Make sure the lyrics for `subject` are cached or on their way: read
    /// them off the UI thread and swap the result in when done. A file
    /// checks its sidecars, the store and its tag; anything else has only
    /// the store, and reads it the same way.
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
                // A different track's sheet reads from the top, not from
                // wherever the previous track's scroll was.
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

    /// Send both faces back to the top and drop the follow glide, for a
    /// sheet that has been swapped out from under them.
    fn rewind(&mut self) {
        self.wrap_scroll.set_offset(Default::default());
        self.text_scroll.set_offset(Default::default());
        self.glide_to = None;
    }

    /// The lyrics to show for `subject`: the edit window's unsaved draft
    /// while one is open on it, otherwise what was loaded. None while a
    /// load is still out or when the subject has no words.
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

    /// Whether `subject` is marked as having no lyrics, from the last load
    /// rather than a fresh look at the store, so the empty face costs no
    /// IO however many frames it paints. False while a load is still out.
    fn marked_for(&self, subject: &Subject) -> bool {
        self.loaded
            .as_ref()
            .is_some_and(|(s, _, marked)| s == subject && *marked)
    }

    /// The version of `raw` the synced face steps through: the same sheet with the
    /// configured rests woven in. Cached by the raw sheet's identity and the
    /// rest knobs, so it rebuilds only when the sheet or a knob changes and
    /// every frame between reuses the woven lines. A reload hands a fresh
    /// pointer, so a re-read never reads through a stale weave.
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

    /// Open the edit window on the shown track: it reads the file's words
    /// into a multi-line input, stamps lines against playback, and saves
    /// back where they came from, then rings the lyrics reload broadcast. One
    /// window per track; a second request focuses the open one.
    fn open_edit(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.resolved.get(self.config.source, &self.state, cx) else {
            return;
        };
        // The editor works on the subject, not the track: lyrics storage
        // is per file for a file, and cue tracks of one image share one.
        // A station with nothing announced has no song to edit yet.
        let Some(target) = self.target(&key, cx).cloned() else {
            return;
        };
        rox_panel_api::openers::lyrics_edit(self.state.clone(), target, cx);
    }

    /// The timestamp `steps` sung lines away from the active one: forward
    /// for a positive step, back for a negative one, clamped to the ends.
    /// None when there is no loaded sheet, no timed lines, or the step
    /// would run off the top during the intro before the first line lights.
    fn walk_lines(&self, steps: i32) -> Option<f64> {
        // The woven sheet, so a step can land on the rests too.
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

    /// Where playback is within `key`, or None when a different track
    /// (or nothing) is playing. The stamp button and the synced highlight
    /// both key off this. The whole key, so a boundary between two tracks of
    /// one image reads as the track change it is.
    fn playback_position(&self, key: &TrackKey, cx: &App) -> Option<f64> {
        self.state
            .player
            .read(cx)
            .now_playing()
            .filter(|now| now.key == *key)
            // A station's own clock counts the listen, which is the
            // evening rather than the song, and a sheet is timed from the
            // song's top. For a file the two are the same number.
            .map(|now| song_clock(now.position_secs, now.song_start_secs))
    }

    /// Whether a pump tick is worth a repaint. Only a synced sheet under a
    /// live playhead is, and only when the track turns over or the lit line
    /// moves. The fade, the word-build, and the glide keep their own frames
    /// once a render runs, so the tick just wakes the panel that was parked
    /// between line changes instead of repainting it 60 times a second.
    fn tick_wakes(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(key) = self.resolved.get(self.config.source, &self.state, cx) else {
            // The source lost its track: repaint to the placeholder if a
            // sheet was up.
            return self.loaded.is_some() || self.pending.is_some();
        };
        // A stream's turnover reads as a different subject down here, which
        // is what wakes the panel for the next song's words.
        let Some(subject) = self.target(&key, cx).map(|t| t.subject.clone()) else {
            return self.loaded.is_some() || self.pending.is_some();
        };
        // A different subject needs a load and a fresh face; let the render
        // kick the fetch. Once it is loading, wait for the load's own notify.
        if self.loaded.as_ref().map(|(s, ..)| s) != Some(&subject) {
            return self.pending.as_ref() != Some(&subject);
        }
        let Some(lyrics) = self.lyrics_for(&subject).cloned() else {
            return false;
        };
        if !lyrics.synced || !self.can_sync(&key, cx) {
            return false;
        }
        // Over the woven sheet, so the compared index matches the one the
        // render stores and the rests count as line changes worth a wake.
        let lyrics = self.display_lyrics(&lyrics);
        let active = self
            .playback_position(&key, cx)
            .and_then(|secs| active_line(&lyrics, secs));
        active != self.active_line
    }

    /// Open the match window on the shown track: it searches online,
    /// ranks candidates by confidence, and saves the one the user
    /// confirms, so nothing is written before a look. The window rings
    /// the lyrics reload broadcast on save.
    fn open_match(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.resolved.get(self.config.source, &self.state, cx) else {
            return;
        };
        let Some(target) = self.target(&key, cx).cloned() else {
            return;
        };
        rox_panel_api::openers::lyrics_matcher(self.state.clone(), target, cx);
    }

    /// Say the shown track has no lyrics: out of the sidecar, the store,
    /// and the embedded tag, and marked so nothing puts them back. Both
    /// halves matter. Clearing alone leaves the next automatic lookup free
    /// to refill it, and marking alone would hide words the file still
    /// has.
    fn wipe(&mut self, cx: &mut Context<Self>) {
        self.set_none(true, cx);
    }

    /// Hand the track back: the mark comes off and the lookups may fill it
    /// again. Nothing to restore, since the wipe was the deletion.
    fn unmark_none(&mut self, cx: &mut Context<Self>) {
        self.set_none(false, cx);
    }

    /// The two above, off the UI thread. Setting wipes first and marks on
    /// the way out, so a failed delete never leaves the track marked with
    /// words still in it.
    fn set_none(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(key) = self.resolved.get(self.config.source, &self.state, cx) else {
            return;
        };
        let Some(subject) = self.target(&key, cx).map(|t| t.subject.clone()) else {
            return;
        };
        // Auto-search runs once per subject, so lifting the mark has to
        // hand this one back to it or the switch reads as one-way.
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
                // Every panel on this track re-reads, the same poke a save
                // from the edit or match window sends.
                cx.update(|cx| rox_panel_api::openers::lyrics_saved(&subject, cx))
                    .ok();
            }
        })
        .detach();
    }

    /// Drop the cached sheet for `path` and repaint, so a save made
    /// outside the panel (the edit or match window, in this panel or any
    /// other) shows on the next render. Lyrics aren't in the projection,
    /// so the lyrics reload broadcast is the panel's only signal to re-read.
    pub fn reload(&mut self, subject: &Subject, cx: &mut Context<Self>) {
        if self.loaded.as_ref().is_some_and(|(s, ..)| s == subject) {
            self.loaded = None;
        }
        cx.notify();
    }

    /// Take the edit window's unsaved draft for `subject`, or None when
    /// that window closed or moved to another track. The draft outranks
    /// what is stored for as long as it stands, so an offset nudge shows
    /// here on the press rather than at the save.
    ///
    /// The faces go back to the top when a draft arrives or leaves, the
    /// same as for any other sheet swap: a stamp pass can change how many
    /// lines there are, and the row the scroll was parked on is not the
    /// row it lands on.
    pub fn set_preview(&mut self, subject: &Subject, text: Option<&str>, cx: &mut Context<Self>) {
        let held = self.preview.as_ref().map(|(at, _)| at.clone());
        if held.as_ref() != Some(subject) && text.is_none() {
            return;
        }
        let draft = text.map(|text| (subject.clone(), Arc::new(sheet(text.to_string()))));
        // A draft arriving or leaving swaps the sheet under the faces; a
        // keystroke inside one that is already up leaves the scroll where
        // it was, or typing would fight the reader for it. Only for the
        // subject this panel is on, so an editor open on another track
        // never jerks it.
        let swapped = draft.as_ref().map(|(at, _)| at) != held.as_ref();
        if swapped && self.showing() == Some(subject) {
            self.rewind();
        }
        self.preview = draft;
        cx.notify();
    }

    /// The subject this panel is on, off the target cache the render
    /// fills. None before the first render and for a station between
    /// announcements.
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

    // The lyric appearance section has its own font picker beside the
    // weight and size knobs, so the shared page leaves off its generic one.
    fn has_own_font(&self) -> bool {
        true
    }

    /// The lyric type controls are on the Appearance page beside the
    /// shared frame and color knobs, the grid's tile-size move: the font
    /// family, weight, and size.
    fn appearance(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        // Reset the lyric type back to its defaults: family off to the app
        // font, weight, size, and spacing to the built-in look.
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

    /// The edit pencil shares the title bar row, the metadata panel's move.
    /// It opens the edit window; hidden while the panel shows no track.
    fn title_suffix(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        // A station between announcements has no song to edit, so the
        // pencil stays off rather than opening on nothing.
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
        // Opens the edit window, the same one the title-bar pencil drives,
        // so a right click gets to it too.
        let weak = cx.entity().downgrade();
        let menu = menu.item(
            PopupMenuItem::new(rox_i18n::t!("lyrics-edit-lyrics"))
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
        // Getting rid of a wrong sheet, and keeping it gone. The two read
        // as one switch and only ever one of them applies: a marked track
        // loads as empty, so a sheet showing means it isn't marked, and
        // the wipe is the thing that marks it.
        //
        // A station between announcements gets neither: there is no song
        // under it yet to wipe or to mark, so the switch would be a
        // control that does nothing.
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
        // The panel is a focus stop: a click puts the keyboard here and
        // tab walks to it, which is also what puts its tab group on the
        // focus path for the tab-cycle chord.
        let focus = self.focus.clone();
        panel::themed(&chrome, || self.body(window, cx).track_focus(&focus))
    }
}

impl LyricsPanel {
    fn body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        // Solo there is no tab bar to host the edit pencil, but a body
        // toolbar just to hold it eats space that reads as chrome; the
        // right-click menu's Edit Lyrics opens the edit window instead.
        // Tabbed, the pencil goes on the tab bar through the title suffix.
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
    /// sheet, or a quiet placeholder. Editing happens in its own window.
    fn content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(key) = self.resolved.get(self.config.source, &self.state, cx) else {
            // With the name showing, a track stands in for the panel's face,
            // so with none the panel reads as empty rather than flashing a
            // "No track" notice.
            return if self.config.show_name {
                div().size_full()
            } else {
                placeholder(rox_i18n::t!("content-no-track"))
            };
        };

        // The subject rides along with the key through the whole face: a
        // stream's song turns over under one URL, and everything below
        // that would otherwise keep showing the song before it. A station
        // that hasn't announced anything has no subject at all, and reads
        // as the wordless track it is.
        let Some(subject) = self.target(&key, cx).map(|t| t.subject.clone()) else {
            return self.empty_face(&key, None, cx);
        };

        self.ensure_loaded(&subject, cx);
        let Some(lyrics) = self.lyrics_for(&subject).cloned() else {
            // Still loading, or the track has none.
            return if self.pending.as_ref() == Some(&subject) {
                loading()
            } else {
                self.empty_face(&key, Some(&subject), cx)
            };
        };

        // A station's first song can't be followed: see [`can_sync`]. Its
        // words still read, stamps and all stripped off by the parse.
        if lyrics.synced && self.can_sync(&key, cx) {
            self.synced_face(&key, &lyrics, window, cx)
        } else {
            self.plain_face(&key, &lyrics, cx)
        }
    }

    /// The empty face: the quiet "no lyrics" line, with the online search
    /// beside or under it while a lyrics provider is enabled and the button
    /// is not hidden. The search opens the match window rather than writing
    /// straight away. The whole face honors the panel's alignment, and once
    /// the panel is too short to stack the line over the button it flows
    /// them onto one row so both still show. Auto-search kicks off here too.
    ///
    /// A track marked as having no lyrics says so instead, and the search
    /// turns into the way back out: the mark is what stops the lookups, so
    /// offering the lookup under it would read as a face arguing with
    /// itself. Lifting the mark hands the track to auto-search anyway.
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
        // Unmeasured (height 0) stacks; only a measured, short panel flows
        // the line and button inline, so the first frame never flickers.
        let inline =
            self.empty_size.height > px(0.) && self.empty_size.height < px(EMPTY_INLINE_MAX_H);
        let marked = subject.is_some_and(|subject| self.marked_for(subject));
        // Nothing to file a sheet under yet, which is a station between
        // announcements: the button would open a window on no song.
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
        // The track's name over the quiet line, so a wordless track still
        // says what it is. Title falls back to the file stem, the artist
        // trailing it when the tags have one.
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
            let notice = if marked {
                rox_i18n::t!("lyrics-marked-notice")
            } else {
                rox_i18n::t!("lyrics-no-lyrics-notice")
            };
            d.child(div().text_color(palette::text_faint()).child(notice))
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
    /// artist when the tags have one, the file stem standing in for a
    /// missing title.
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

    /// With auto-search on, look the shown track up online in the
    /// background the first time its empty face paints, and save the top
    /// match when it clears [`AUTO_SAVE_CONFIDENCE`]. A weak match is left
    /// alone for the manual search, which shows every candidate. Runs once
    /// per track so a repaint never re-queries.
    ///
    /// A station's song is the same lookup under the song it announced, so
    /// every song of a broadcast gets its own look instead of the first
    /// one taking the station's only turn, and what comes back is filed in
    /// the store under that song. It is there the next time the song comes
    /// round, on that station or any other.
    ///
    /// A track marked as having no lyrics is skipped: the mark is there
    /// precisely because a lookup got it wrong, and this search is what
    /// would otherwise put the wrong sheet back every session.
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

    /// The synced face: one row per timed line, the line under the
    /// playhead lit and gliding to the middle while follow is on, with the
    /// optional fade-in and word-build effects layered on. Clicking a line
    /// seeks to it. Blank pad rows at the ends let the first and last lines
    /// reach the middle too when pre-scroll is on.
    fn synced_face(
        &mut self,
        key: &TrackKey,
        lyrics: &Arc<Lyrics>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        // Step through the woven sheet, not the raw one: the rests are real
        // timed lines to the follow, the highlight, and the scroll step.
        let lyrics = self.display_lyrics(lyrics);
        // The playhead only applies to the track it's on; a Selected
        // source pointed elsewhere reads no position and just scrolls.
        let position = self.playback_position(key, cx);
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

        // The read head: how far into the active line's text the playhead
        // has sung, so line_rows knows where to cut it. A sheet that times
        // its words drives this off that clock; one that doesn't spreads
        // the text across the span the next timed line closes.
        self.head = None;
        if self.config.word_by_word
            && let (Some(pos), Some(ix)) = (position, active)
        {
            let line = &lyrics.lines[ix];
            let until = lyrics.lines[ix + 1..].iter().find_map(|line| line.at);
            self.head = Some(lyrics::read_head(line, pos, until));
        }
        // The read head tracks the playhead across the line, so keep the
        // frames coming while it still has line left to run; the pump's
        // tick no longer wakes the panel between line changes. Paused, the
        // head sits still and asking for another frame would just rebuild
        // the sheet sixty times a second to draw the same thing; the
        // player's own change wakes the panel again on resume.
        animating |= self.state.player.read(cx).is_playing()
            && match (self.head, active) {
                (Some(head), Some(ix)) => head < lyrics.lines[ix].text.len(),
                _ => false,
            };

        // Pad the ends so the first and last lines can center as well.
        self.pad = self.pad_height(line_height(self.config.font_size, self.config.line_spacing));

        // Re-aim the glide when the active line moves; drive it toward the
        // middle here in render, the grid's follow idiom, asking for the
        // next frame only while it still moves.
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
                // Before the sheet's first layout there is nothing to
                // measure against; hold the glide for the next frame.
                None => false,
            };
            // A shorter sheet can strand the target past its last line,
            // which would never measure and never arrive. Drop the glide
            // instead of asking for frames forever.
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
                div()
                    .id("lyrics-lines")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.wrap_scroll)
                    .flex()
                    .flex_col()
                    // The pads are children 0 and last, so a line at index
                    // ix is child ix + 1, which is what center_target
                    // measures. They stay in the tree at zero height with
                    // pre-scroll off so that offset never moves.
                    .child(div().flex_none().h(pad))
                    .children(rows)
                    .child(div().flex_none().h(pad)),
            )
    }

    /// Where the scroll should sit to center line `ix`, read off the
    /// bounds the sheet actually laid that row out at. None before the
    /// first layout, and while the row hasn't been measured.
    ///
    /// Rows wrap, so one line can be twice another's height and a stride
    /// estimate would put the active line off center or off screen. The
    /// handle records every child's bounds in window space; the content
    /// offset is that minus where the viewport starts, minus how far it is
    /// already scrolled.
    fn center_target(&self, ix: usize) -> Option<Pixels> {
        let view = self.wrap_scroll.bounds();
        let item = self.wrap_scroll.bounds_for_item(ix + 1)?;
        let origin = item.top() - view.top() - self.wrap_scroll.offset().y;

        panel::glide_target_at(&self.wrap_scroll, Axis::Vertical, origin, item.size.height)
    }

    /// The pad each end of the synced sheet carries so the first and last
    /// lines can still glide to the middle: half the viewport when
    /// pre-scroll is on, nothing when it is off. A screenful of rows
    /// stands in before the first layout gives the scroll a viewport, so
    /// the opening frame is close rather than jumping once measured.
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

    /// The synced sheet's rows: each timed line, the one under the
    /// playhead lit and the rest muted and clickable to seek. Full width,
    /// so the alignment knob actually centers the text.
    ///
    /// Every line is built, not a window of them. A sheet is a few dozen
    /// lines and rows wrap to as many visual lines as they need, so
    /// there's no stride to virtualize against; the scroll measures what
    /// it laid out and the glide centers off those real bounds.
    fn line_rows(&mut self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        // The woven sheet synced_face built this frame, rests and all.
        let Some(lyrics) = self.display_arc().cloned() else {
            return Vec::new();
        };
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
        // The unsung half of the active line under the fill: the lit color
        // pulled toward the muted one by the knob, so 0 leaves the whole
        // line bright and 1 sinks the tail to where a passed line sits.
        let unsung = palette::mix(palette::text_bright(), palette::text_muted(), word_dim);
        // Before the first line lights up, still measure the falloff from
        // where the read head is headed (the first timed line) so a live
        // intro shows the sheet already dimmed toward the edge instead of
        // sitting flat until the first word passes. With no playhead on the
        // track there's nothing to anchor to, so the sheet reads whole.
        let falloff_from = active.or_else(|| {
            positioned.then(|| {
                lyrics
                    .lines
                    .iter()
                    .position(|line| line.at.is_some())
                    .unwrap_or(0)
            })
        });

        (0..lyrics.lines.len())
            .map(|ix| {
                let line = &lyrics.lines[ix];
                let at = line.at;
                let is_active = Some(ix) == active;
                let text = &line.text;
                // Paused off a playhead there's no head to run, and the
                // line reads whole like any other.
                let running = is_active && word_by_word && !text.is_empty();

                let content: AnyElement = match (running.then_some(head).flatten(), word_style) {
                    // The karaoke fill: one wrapping text layout with the
                    // unsung tail colored back. Splitting it into two
                    // elements instead would break the wrap and stop the
                    // cut landing inside a word, which is the whole look.
                    (Some(head), WordStyle::Fill) => gpui::StyledText::new(text.clone())
                        .with_highlights([(
                            head..text.len(),
                            gpui::HighlightStyle {
                                color: Some(unsung.into()),
                                ..Default::default()
                            },
                        )])
                        .into_any_element(),

                    // The older build: each word waits out of sight, then
                    // fades and rises into place as the head crosses it.
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

                // With the reveal on, every line past the active one (and
                // every line during the intro) waits invisible until its
                // turn, its row still holding the space. Off, the whole
                // sheet reads and the falloff does the dimming. Either way
                // a sheet viewed with no playhead on it reads whole.
                let upcoming = self.config.hide_upcoming
                    && positioned
                    && active.is_none_or(|active| ix > active);

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
                        .w_full()
                        .flex()
                        .flex_none()
                        .items_center()
                        // Wrapping rows size to their content and lead
                        // through the text's own line height; fixed rows
                        // keep the one height they always had and clip.
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
                // A timed line seeks to its own time on click. The line
                // that's already playing is where we are, so clicking it
                // does nothing rather than yanking the song back to its
                // start.
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
            .collect()
    }

    /// The plain face: the whole sheet as wrapped text on its own scroll,
    /// no highlight, no follow, for lyrics with no timestamps. With the
    /// title option on, the track name pins above the scroll so a short
    /// panel still says what song it is.
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
            // The spacing knob sets the text line height here, so it moves
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
/// edge. Full when there's no active line to measure from, when the
/// factor is zero, or when this line is on the edge that doesn't dim.
fn falloff(dim: f32, edge: DimEdge, active: Option<usize>, ix: usize) -> f32 {
    let Some(active) = active else {
        return 1.0;
    };
    if !edge.dims(ix < active) {
        return 1.0;
    }
    curve::falloff(dim, ix.abs_diff(active) as u32)
}

/// The lyric text's alignment as the text system spells it.
fn text_align(align: Align) -> gpui::TextAlign {
    match align {
        Align::Left => gpui::TextAlign::Left,
        Align::Center => gpui::TextAlign::Center,
        Align::Right => gpui::TextAlign::Right,
    }
}

/// The byte ranges of `text`'s whitespace-separated words, in order. The
/// per-word build needs where each word sits, not a copy of it, so the
/// read head's byte offset can be measured against the same string.
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

/// How far the read head at `head` has crossed the word spanning `span`,
/// 0 before it and 1 once past. The build fades a word in over the first
/// [`WORD_FADE`] of its own span, so a short word still reads as a beat
/// rather than a blink.
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

/// A sheet held in memory rather than read from anywhere: the same parse
/// [`lyrics::load`] runs, over the edit window's unsaved draft.
///
/// The source names where an edit would save back, and nothing saves from
/// a draft the editor still owns, so [`lyrics::Source::Tag`] here is the
/// field going unread rather than a destination anything will use.
fn sheet(text: String) -> Lyrics {
    let (lines, synced) = lyrics::parse(&text);

    Lyrics {
        source: lyrics::Source::Tag,
        text,
        lines,
        synced,
    }
}

/// A quiet centered line in place of the sheet.
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
