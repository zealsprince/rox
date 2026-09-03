//! The genre tagger: the one window in rox that asks for a single tag,
//! one track at a time, with the track playing while it asks. Genre is
//! the field a scanner can't infer and a provider often disagrees about,
//! so filling it is a listening job, not a lookup job. The window's whole
//! shape follows from that: play the track, rank the likely answers under
//! it with the evidence beside each, take a typed answer for everything
//! the ranking missed, write, and move on. Nothing here batches silently,
//! and nothing here guesses on the user's behalf. A track that is two
//! genres gets them by collecting: Shift with a digit, or a Shift-click,
//! adds a row's genre to the box rather than writing it, and Enter writes
//! the list in the library's "; " spelling.
//!
//! It has two ways of choosing what it asks about. Opened cold it looks at
//! whatever is playing, tagged or not, and offers to retag it: the window
//! never touches the transport on its own, so opening it beside a track
//! you're already listening to is a way to ask "what would you call this".
//! "Begin queue" is the other way: from then on it walks every track with
//! no genre, plays each as it arrives, and steps on with every write or
//! skip. Stopping the queue drops back to watching the player.
//!
//! The suggestions come from [`rox_library::genre_suggest`], which votes
//! over three sources that disagree in useful ways: what the rest of the
//! album already says, what the rest of the artist already says, and what
//! the acoustically nearest neighbours say. Last.fm's artist tags join as
//! a fourth source, but only when asked: a tagging pass moves at the speed
//! of the keyboard, and a network round trip per track would turn a
//! five-minute session into a twenty-minute one, so the lookup is a button
//! rather than a step.
//!
//! Writes go through the same atomic copy-verify-rename layer as every
//! other tag edit (ADR 4), one file at a time with per-file isolation, and
//! a cue subsong is refused rather than written, because there is nowhere
//! inside a shared image that means "track 4". The album switch is the one
//! multi-file action: in the queue it reaches the album's other untagged
//! rows, and on a track being retagged it reaches the whole album, both
//! scoped to rows sharing an album name and a folder, so a compilation
//! split across directories can't be painted with one word. One level of
//! undo puts every file the last write touched back to what it held.
//!
//! The window never patches the library's projection: it holds an Arc of
//! the one it opened over, and rebuilds its walk from scratch when the
//! catalog swaps a new one in, keeping the cursor on the same track where
//! that track survives.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gpui::{
    div, img, prelude::*, px, size, svg, AnyElement, App, AsyncApp, Bounds, ClickEvent, Context,
    Div, Entity, FocusHandle, Global, KeyDownEvent, ObjectFit, ScrollHandle, SharedString,
    Stateful, Subscription, Window, WindowHandle,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::Scrollbar;
use gpui_component::spinner::Spinner;
use gpui_component::{Root, Sizable, Size};

use rox_core::fmt::fmt_ms;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_library::cue::TrackKey;
use rox_library::genre;
use rox_library::genre_suggest::{self, Suggestion};
use rox_library::projection::Projection;
use rox_library::store;
use rox_library::writer::{self, Change, Field};
use rox_net::providers;
use rox_panel_api::panel::{self, AppState};
use rox_panel_api::suggest;
use rox_panel_kit::ui::{checkbox, section, small_button, MIN_SIZE};
use rox_services::backdrop::WindowBackdrop;
use rox_services::catalog::LibraryEvent;
use rox_services::thumbs::Thumb;

/// How many suggestions the vote is asked for. Eight because the digit row
/// picks them: a ninth row would be a row with no key behind it.
const ROW_CAP: usize = 8;

/// The current track's cover. Big enough to recognize a sleeve at a
/// glance, small enough that the ranking stays above the fold at the
/// window's minimum height.
const COVER: f32 = 88.;

/// Where in the track playback starts. A third of the way in skips the
/// intro, which is the part of a song that says least about its genre.
const START_DIVISOR: u32 = 3;

/// The ceiling on that offset. Without it a twenty-minute mix would start
/// seven minutes in, well past the point where the answer was already
/// obvious, and the pass would spend its time seeking.
const START_CAP_MS: u32 = 30_000;

/// A row's share bar. Thin: the number beside it is the real reading, and
/// the bar is there to make the ranking visible without being read.
const BAR_W: f32 = 56.;
const BAR_H: f32 = 3.;

/// How many of Last.fm's top tags a lookup keeps. The list is ordered by
/// how often listeners applied each tag, and past the first few it turns
/// into "seen live" and "favourites", which are not genres.
const LOOKUP_TAGS: usize = 5;

/// The open tagger, if any. One at a time, the duplicates window's rule:
/// a walk in progress with a write in flight isn't worth losing to a
/// second copy, so asking again brings this one forward.
#[derive(Default)]
struct OpenTagger(Option<WindowHandle<Root>>);

impl Global for OpenTagger {}

/// Open the genre tagger, or bring the open one forward.
pub fn open(state: AppState, cx: &mut App) {
    if let Some(handle) = cx.try_global::<OpenTagger>().and_then(|o| o.0) {
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let bounds = Bounds::centered(None, size(px(720.), px(640.)), cx);
    let handle = rox_panel_api::panel::open_child_window(
        cx,
        rox_i18n::t!("tag-genres-window-title"),
        bounds,
        Some(MIN_SIZE),
        move |window, cx| cx.new(|cx| GenreTagger::new(state, window, cx)),
    );
    cx.set_global(OpenTagger(Some(handle)));
}

/// One track waiting for a genre: its database id (the identity that
/// survives a projection swap), the row it sits at in the projection the
/// walk was built from, and the two symbols the album switch groups on.
///
/// `album` is None where the row carries no album name at all. A folder of
/// loose singles all share the empty album, and grouping on it would let
/// one click paint a dozen unrelated tracks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pending {
    id: i64,
    row: u32,
    album: Option<u32>,
    folder: u32,
    /// Which subsong of its file the row is. Non-zero means a cue track,
    /// which has no place on disk to write to.
    sub: u16,
}

/// Which way the window is choosing its subject.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Whatever the player has on, for retagging. The transport is the
    /// user's, untouched.
    Watching,
    /// Walking the untagged list, playing each track as it comes up.
    Queue,
}

/// One file a write touched, with what it held before, so the write can
/// be taken back exactly. `walk` says the row was one of the untagged walk's
/// own, so the write took it out and taking the write back puts it back;
/// false for a retag, which removes nothing from the walk.
#[derive(Clone, Debug)]
struct Written {
    walk: bool,
    pending: Pending,
    key: TrackKey,
    before: String,
}

pub struct GenreTagger {
    state: AppState,
    /// The projection the walk was built over, held whole. Suggestions run
    /// against this one, so a swap mid-vote can't shift the rows out from
    /// under a result that's already on its way back.
    projection: Option<Arc<Projection>>,
    mode: Mode,
    /// The untagged walk, in projection order, and where the queue is in it.
    items: Vec<Pending>,
    pos: usize,
    /// What the window is asking about right now: in the queue, the row at
    /// the cursor; watching, the playing track once it's found in the
    /// projection. None when there's nothing to ask about.
    subject: Option<Pending>,
    /// The subject's key, resolved once per seat: the cover, the playback,
    /// and the write all address through it.
    key: Option<TrackKey>,
    /// The subject's genre as it stands, for the card. Empty in the queue
    /// by definition.
    before: String,
    suggestions: Vec<Suggestion>,
    loading: bool,
    /// Bumped on every seat. A batch of suggestions carrying an older
    /// number belongs to a track the window has already left, so it drops.
    generation: u64,
    input: Entity<InputState>,
    typed: String,
    /// A failed write, or the note that a cue track can't take one.
    error: Option<SharedString>,
    applying: bool,
    /// How far the write in flight has got: files done, files in all. An
    /// album sweep touches a dozen files one after another, and the footer
    /// counts them off so the window doesn't look stuck.
    progress: (usize, usize),
    /// Raised when the window goes away. A sweep can have a dozen files
    /// left to write, and the user closing the window is the user saying
    /// stop, so the loop reads this between files.
    cancel: Arc<AtomicBool>,
    undo: Option<Vec<Written>>,
    /// Whether an answer also lands on the album's siblings. Sticky across
    /// tracks: a user working through a folder of albums decides this
    /// once, and Ctrl with a digit is the per-pick override.
    album_too: bool,
    /// What the Last.fm lookup answered for this track's artist, fed to the
    /// vote as its fourth source. Empty until the button is pressed.
    lookup: Vec<String>,
    looking_up: bool,
    /// Whether every seat asks Last.fm on its own, so the fourth source
    /// is there without a press per track. Sticky for the session like
    /// the album switch; the button and L still ask by hand.
    auto_lookup: bool,
    /// What the lookup said, in words, beside the button.
    lookup_note: Option<SharedString>,
    /// The ranking's scroll, for a window shorter than eight rows.
    scroll: ScrollHandle,
    focus: FocusHandle,
    backdrop: WindowBackdrop,
    /// This window pumps its own frames, so the backdrop needs its own
    /// wake on a new bake.
    _backdrop_changed: Subscription,
    _library_changed: Subscription,
    /// Follows the player: repaints the transport's play/pause face, and
    /// while watching, moves the subject onto whatever starts playing.
    _player_changed: Subscription,
    _input_events: Vec<Subscription>,
}

impl GenreTagger {
    fn new(state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        let _player_changed = cx.observe_in(&state.player, window, |this, _, window, cx| {
            this.follow_player(window, cx);
            cx.notify();
        });
        let _library_changed = cx.subscribe_in(
            &state.library,
            window,
            |this: &mut Self, _, event: &LibraryEvent, window, cx| {
                if matches!(event, LibraryEvent::Updated) {
                    this.rebuild(window, cx);
                }
            },
        );
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx)
                .placeholder(rox_i18n::t!("tag-genres-input-placeholder"));
            input.lsp.completion_provider = suggest::provider(&state.library, &Field::Genre, cx);
            input
        });
        let _input_events = vec![cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, input, event: &InputEvent, window, cx| match event {
                InputEvent::Change => {
                    this.typed = input.read(cx).value().trim().to_string();
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {
                    let typed = this.typed.clone();
                    let album = this.album_too;
                    this.apply(typed, album, window, cx);
                }
                _ => {}
            },
        )];
        let mut this = GenreTagger {
            state,
            projection: None,
            mode: Mode::Watching,
            items: Vec::new(),
            pos: 0,
            subject: None,
            key: None,
            before: String::new(),
            suggestions: Vec::new(),
            loading: false,
            generation: 0,
            input,
            typed: String::new(),
            error: None,
            applying: false,
            progress: (0, 0),
            cancel: Arc::new(AtomicBool::new(false)),
            undo: None,
            album_too: false,
            lookup: Vec::new(),
            looking_up: false,
            auto_lookup: true,
            lookup_note: None,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            backdrop: WindowBackdrop::default(),
            _backdrop_changed,
            _library_changed,
            _player_changed,
            _input_events,
        };
        this.rebuild(window, cx);
        this
    }

    /// Build the walk from the catalog's current projection, keeping the
    /// cursor where it can. The track under the cursor is looked up by
    /// database id in the new list; where it's gone (this window just
    /// tagged it) the index stays put, which lands on whatever slid up into
    /// its place. Watching, the subject is re-found the same way, by id.
    fn rebuild(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(projection) = self.state.library.read(cx).projection().cloned() else {
            self.projection = None;
            self.items.clear();
            self.subject = None;
            cx.notify();
            return;
        };
        let holding = self.items.get(self.pos).map(|p| p.id);
        let items = build_queue(&projection);
        let pos = holding
            .and_then(|id| items.iter().position(|p| p.id == id))
            .unwrap_or(self.pos.min(items.len().saturating_sub(1)));
        let first = self.projection.is_none();
        self.projection = Some(projection);
        self.items = items;
        self.pos = pos;
        match self.mode {
            Mode::Queue => {
                // Only re-seat when the cursor actually landed somewhere
                // else. A rebuild that finds the same track under the cursor
                // must not restart it: the catalog fires on every rating
                // click and scrobble, and a walk that jumped back to the
                // intro on each one would be unusable.
                let moved = first || holding != self.items.get(pos).map(|p| p.id);
                if moved {
                    self.seat(window, cx);
                } else {
                    cx.notify();
                }
            }
            Mode::Watching => {
                // The playing track's row may have moved, and its genre may
                // be what this window just wrote. Refresh what the card
                // shows without re-asking the vote or clearing the box.
                let subject = self.playing_subject(cx);
                if subject.map(|p| p.id) != self.subject.map(|p| p.id) {
                    self.seat(window, cx);
                } else {
                    self.subject = subject;
                    self.before = self.genre_of(subject);
                    cx.notify();
                }
            }
        }
    }

    /// The playing track as a walk entry, when it's a library track the
    /// projection knows. A stream, or a file the library hasn't indexed,
    /// gives None.
    fn playing_subject(&self, cx: &App) -> Option<Pending> {
        let projection = self.projection.as_ref()?;
        let key = self.state.player.read(cx).now_playing()?.key;
        let id = self.state.library.read(cx).id_for_key(&key)?;
        let row = projection.db_id.iter().position(|&db| db == id)? as u32;
        Some(pending_at(projection, row))
    }

    /// The subject's genre string as the projection holds it.
    fn genre_of(&self, subject: Option<Pending>) -> String {
        match (self.projection.as_ref(), subject) {
            (Some(projection), Some(p)) => projection.resolve(p.row).genre.to_string(),
            _ => String::new(),
        }
    }

    /// While watching, move onto whatever the player just switched to.
    /// Compared by id so the position ticks the player notifies on don't
    /// re-seat the same track over and over.
    fn follow_player(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode != Mode::Watching {
            return;
        }
        let subject = self.playing_subject(cx);
        if subject.map(|p| p.id) != self.subject.map(|p| p.id) {
            self.seat(window, cx);
        }
    }

    /// Take up the subject for the mode: resolve its key, clear the last
    /// track's answers, play it if the queue is running, and ask for
    /// suggestions. The album switch and the box's focus are left alone,
    /// since both belong to the user rather than the track.
    fn seat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        self.suggestions.clear();
        self.loading = false;
        self.error = None;
        self.typed.clear();
        self.lookup.clear();
        self.looking_up = false;
        self.lookup_note = None;
        self.input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.subject = match self.mode {
            Mode::Queue => self.items.get(self.pos).copied(),
            Mode::Watching => self.playing_subject(cx),
        };
        self.before = self.genre_of(self.subject);
        let Some(item) = self.subject else {
            self.key = None;
            cx.notify();
            return;
        };
        self.key = self
            .state
            .library
            .read(cx)
            .keys_for(&[item.id])
            .ok()
            .and_then(|mut keys| keys.pop());
        if !writer::writes_to_file(item.sub) {
            self.error = Some(rox_i18n::t!("tag-genres-unwritable"));
        }
        if self.mode == Mode::Queue {
            self.play(cx);
        }
        self.request(window, cx);
        if self.auto_lookup {
            self.look_up(window, cx);
        }
        cx.notify();
    }

    /// Begin queue: from here the window chooses what plays. Picks up
    /// from wherever the cursor was, so stopping and starting again
    /// doesn't send the walk back to the top.
    fn begin_queue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        self.mode = Mode::Queue;
        self.pos = self.pos.min(self.items.len() - 1);
        self.seat(window, cx);
    }

    /// Stop queue: back to watching the player. The track the queue was on
    /// keeps playing, and it's what the window now watches, so nothing
    /// visibly changes except which controls are offered.
    fn stop_queue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = Mode::Watching;
        self.seat(window, cx);
    }

    /// Play the subject from partway in. The seek rides the same command
    /// channel the session start queues on, so it lands ahead of the
    /// engine's first decode rather than racing it from a later frame.
    ///
    /// The player exposes no "play from here": its one start-position
    /// parameter comes up paused (it exists for the launch restore), and
    /// unpausing that would be the same two commands with an extra stop in
    /// the middle.
    fn play(&mut self, cx: &mut Context<Self>) {
        let (Some(key), Some(item)) = (self.key.clone(), self.subject) else {
            return;
        };
        let start = self
            .projection
            .as_ref()
            .and_then(|p| p.duration_ms.get(item.row as usize).copied())
            .map(|ms| (ms / START_DIVISOR).min(START_CAP_MS))
            .unwrap_or(0);
        self.state.player.update(cx, |player, cx| {
            player.play_explicit(vec![key], cx);
            if start > 0 {
                player.seek_to(start as f64 / 1000.0);
            }
        });
    }

    /// Ask the vote for this track's likely genres, off the UI thread. The
    /// read connection is opened per request rather than held: a tagging
    /// pass makes one of these every few seconds, and the open is cheap
    /// beside the nearest-neighbour query it precedes.
    fn request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(projection), Some(item)) = (self.projection.clone(), self.subject) else {
            return;
        };
        let generation = self.generation;
        self.loading = true;
        let lookup = self.lookup.clone();
        let model = rox_services::acoustic::acoustic_source().id().to_string();
        let db_path = rox_core::settings::data_dir().join("library.db");
        cx.spawn_in(window, async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn(async move {
                    let Ok(conn) = store::open(&db_path) else {
                        return Vec::new();
                    };
                    genre_suggest::suggest(&conn, &model, &projection, item.row, &lookup, ROW_CAP)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.suggestions = found;
                this.loading = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Ask Last.fm what the artist is tagged as, and vote again with the
    /// answer in. Artist-level, since that's the read the service offers
    /// without an account; a compilation's lookup is the compilation's
    /// credited artist. Honours the same provider switch the biography
    /// panel does, and says so instead of silently doing nothing when the
    /// switch is off.
    fn look_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.looking_up {
            return;
        }
        let (Some(projection), Some(item)) = (self.projection.as_ref(), self.subject) else {
            return;
        };
        let artist = projection.resolve(item.row).artist.trim().to_string();
        if artist.is_empty() {
            self.lookup_note = Some(rox_i18n::t!("tag-genres-lookup-none", artist = artist));
            cx.notify();
            return;
        }
        if !providers::artist_online() {
            self.lookup_note = Some(rox_i18n::t!("tag-genres-lookup-off"));
            cx.notify();
            return;
        }
        let generation = self.generation;
        self.looking_up = true;
        self.lookup_note = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let artist = artist.clone();
                    async move { providers::lastfm::artist_info(&artist, "en") }
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                if this.generation != generation {
                    return;
                }
                this.looking_up = false;
                match result {
                    Ok(Some(info)) if !info.tags.is_empty() => {
                        // Last.fm hands its tags over in lowercase; the
                        // library keeps them capitalized.
                        let tags: Vec<String> = info
                            .tags
                            .iter()
                            .take(LOOKUP_TAGS)
                            .map(|tag| genre::capitalize(tag))
                            .collect();
                        this.lookup_note = Some(rox_i18n::t!(
                            "tag-genres-lookup-found",
                            artist = info.name,
                            tags = tags.join(", ")
                        ));
                        this.lookup = tags;
                        this.request(window, cx);
                    }
                    Ok(_) => {
                        this.lookup_note =
                            Some(rox_i18n::t!("tag-genres-lookup-none", artist = artist));
                    }
                    Err(e) => {
                        this.lookup_note = Some(e.into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Move the cursor without writing anything. Queue only: watching has
    /// no list to move along.
    fn skip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode != Mode::Queue || self.applying || self.items.is_empty() {
            return;
        }
        self.pos = (self.pos + 1).min(self.items.len() - 1);
        self.seat(window, cx);
    }

    /// The digit keys: the nth suggestion, with Ctrl reaching the album
    /// whatever the switch says.
    fn pick(&mut self, n: usize, album: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(genre) = self.suggestions.get(n).map(|s| s.genre.clone()) else {
            return;
        };
        self.apply(genre, album, window, cx);
    }

    /// Collect a genre into the box instead of writing it, for a track that
    /// is more than one thing. Joined in the library's "; " spelling, with a
    /// value already in the box left as it was rather than doubled.
    fn add_to_box(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let value = value.trim();
        if value.is_empty() {
            return;
        }
        let have =
            genre::split(&self.typed).any(|part| rox_i18n::fold(part) == rox_i18n::fold(value));
        let joined = if have {
            genre::canonical(&self.typed)
        } else {
            genre::join(genre::split(&self.typed).chain(std::iter::once(value)))
        };
        self.typed = joined.clone();
        self.input
            .update(cx, |input, cx| input.set_value(joined, window, cx));
        cx.notify();
    }

    /// Which rows an answer lands on: the subject alone, or the subject
    /// with its album. In the queue the album is its other untagged rows;
    /// watching, where the subject is being retagged, it's every row of the
    /// album, since "tag the whole album" means the whole album.
    fn targets(&self, album: bool) -> Vec<(bool, Pending)> {
        let Some(subject) = self.subject else {
            return Vec::new();
        };
        if !album {
            return vec![(self.mode == Mode::Queue, subject)];
        }
        match self.mode {
            Mode::Queue => album_peers(&self.items, self.pos)
                .into_iter()
                .filter_map(|i| self.items.get(i).map(|p| (true, *p)))
                .collect(),
            Mode::Watching => self
                .projection
                .as_ref()
                .map(|projection| {
                    album_rows(projection, subject)
                        .into_iter()
                        .map(|row| (false, pending_at(projection, row)))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// How many siblings the album switch would reach beyond the subject,
    /// for the switch's label.
    fn siblings(&self) -> usize {
        self.targets(true).len().saturating_sub(1)
    }

    fn apply(&mut self, genre: String, album: bool, window: &mut Window, cx: &mut Context<Self>) {
        let genre = genre::capitalize(&genre);
        if self.applying || genre.is_empty() {
            return;
        }
        let Some(subject) = self.subject else { return };
        if !writer::writes_to_file(subject.sub) {
            self.error = Some(rox_i18n::t!("tag-genres-unwritable"));
            cx.notify();
            return;
        }
        // A cue row caught in an album sweep drops out of it rather than
        // stopping the sweep: the rest of the album is still writable, and
        // in the queue the row stays in the walk to be refused on its own
        // turn.
        let targets: Vec<(bool, Pending)> = self
            .targets(album)
            .into_iter()
            .filter(|(_, p)| writer::writes_to_file(p.sub))
            .collect();
        if targets.is_empty() {
            return;
        }
        let library = self.state.library.read(cx);
        let projection = self.projection.clone();
        let jobs: Vec<Written> = targets
            .into_iter()
            .filter_map(|(walk, pending)| {
                let key = library.keys_for(&[pending.id]).ok()?.pop()?;
                let before = projection
                    .as_ref()
                    .map(|p| p.resolve(pending.row).genre.to_string())
                    .unwrap_or_default();
                Some(Written {
                    walk,
                    pending,
                    key,
                    before,
                })
            })
            .collect();
        if jobs.is_empty() {
            self.error = Some(rox_i18n::t!("tag-genres-no-file"));
            cx.notify();
            return;
        }
        self.applying = true;
        self.error = None;
        cx.notify();
        self.commit(jobs, Some(genre), window, cx);
    }

    /// Put `genre` on every one of `jobs` (None clears it), then fold the
    /// results back in: in the queue, written rows leave the walk and the
    /// cursor lands on whatever follows; watching, the subject stays and
    /// the card catches up when the catalog reloads. The first failure
    /// shows inline. `undo` says whether this write is one to remember or
    /// the taking-back of one.
    fn commit(
        &mut self,
        jobs: Vec<Written>,
        genre: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let taking_back = genre.is_none();
        self.progress = (0, jobs.len());
        let cancel = self.cancel.clone();
        // Held apart from the window: the sweep has to be able to tell the
        // catalog about files it already wrote even when the window that
        // started it is gone.
        let library = self.state.library.clone();
        cx.spawn_in(window, async move |this, cx| {
            let mut written: Vec<(Written, Change)> = Vec::new();
            let mut failure: Option<SharedString> = None;
            for job in jobs {
                // Closing the window mid-sweep stops it at the next file
                // rather than writing the rest of an album into a window
                // nobody is looking at.
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                // Taking back restores each file's own previous value,
                // which for a queue row is nothing at all.
                let value = match &genre {
                    Some(genre) => Some(genre.clone()),
                    None => (!job.before.is_empty()).then(|| job.before.clone()),
                };
                let change = Change {
                    field: Field::Genre,
                    value,
                };
                let result = cx
                    .background_executor()
                    .spawn({
                        let key = job.key.clone();
                        let change = change.clone();
                        async move { writer::commit_key(&key.path, key.sub, &[change], &[]) }
                    })
                    .await;
                this.update(cx, |this, cx| {
                    this.progress.0 += 1;
                    cx.notify();
                })
                .ok();
                match result {
                    Ok(()) => written.push((job, change)),
                    Err(e) => {
                        if failure.is_none() {
                            let name = job
                                .key
                                .path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| job.key.path.display().to_string());
                            failure = Some(
                                rox_i18n::t!("tag-genres-write-error", name = name, error = e)
                                    .to_string()
                                    .into(),
                            );
                        }
                    }
                }
            }
            // The library first, through the app rather than the window.
            // Files are already changed on disk by this point, and a window
            // closed mid-sweep must not be the reason the database and the
            // projection never hear about them.
            //
            // One batch, one reindex: folding each file back on its own
            // would hit the catalog's busy gate from the second file on and
            // leave the rest of an album stale.
            if !written.is_empty() {
                let edits: Vec<writer::Edit> = written
                    .iter()
                    .map(|(job, change)| writer::Edit {
                        path: job.key.path.clone(),
                        changes: vec![change.clone()],
                        pictures: Vec::new(),
                    })
                    .collect();
                let subs: Vec<u16> = written.iter().map(|(job, _)| job.key.sub).collect();
                let app: &AsyncApp = cx;
                app.update(|cx| {
                    library.update(cx, |library, cx| library.apply_edits(&edits, &subs, cx));
                })
                .ok();
            }
            // Then the window, which is allowed to be gone.
            this.update_in(cx, |this, window, cx| {
                this.applying = false;
                this.error = failure;
                if written.is_empty() {
                    cx.notify();
                    return;
                }
                if taking_back {
                    // Undone rows the write took out of the walk go back
                    // into it in projection order, and the cursor returns
                    // to the first of them.
                    let entries: Vec<Pending> = written
                        .iter()
                        .filter_map(|(job, _)| job.walk.then_some(job.pending))
                        .collect();
                    if !entries.is_empty() {
                        this.pos = reinsert(&mut this.items, &entries);
                    }
                    if this.mode == Mode::Queue {
                        this.seat(window, cx);
                    } else {
                        cx.notify();
                    }
                    return;
                }
                let applied: Vec<Pending> = written
                    .iter()
                    .filter_map(|(job, _)| job.walk.then_some(job.pending))
                    .collect();
                this.undo = Some(written.into_iter().map(|(job, _)| job).collect());
                if this.mode == Mode::Queue && !applied.is_empty() {
                    this.pos = after_apply(&mut this.items, &applied);
                    this.seat(window, cx);
                } else {
                    // Rows the retag reached leave the untagged walk even
                    // though the cursor isn't on it, so a later Begin queue
                    // doesn't ask about them again.
                    let gone: HashSet<i64> = this
                        .undo
                        .iter()
                        .flatten()
                        .map(|job| job.pending.id)
                        .collect();
                    this.items.retain(|p| !gone.contains(&p.id));
                    this.pos = this.pos.min(this.items.len().saturating_sub(1));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Take the last write back: every file it touched returns to what it
    /// held, queue rows go back in the walk where they were, and the cursor
    /// returns to the first of them.
    fn undo_last(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.applying {
            return;
        }
        let Some(jobs) = self.undo.take() else { return };
        self.applying = true;
        self.error = None;
        cx.notify();
        self.commit(jobs, None, window, cx);
    }

    /// The keys the window answers to on its own root, not through the
    /// keymap: a tagging pass is a mode with its own vocabulary, and
    /// binding digits app-wide to pick a row would be absurd anywhere
    /// else. Everything but Escape only fires while the root itself holds
    /// focus, so typing "1990s Rock" in the box types it.
    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;
        if key == "escape" {
            self.input
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.typed.clear();
            cx.notify();
            return;
        }
        if !self.focus.is_focused(window) {
            return;
        }
        if key == "z" && mods.control {
            self.undo_last(window, cx);
            return;
        }
        if mods.alt || mods.platform {
            return;
        }
        // On Linux, gpui hands Shift+1 over as "!" with shift cleared, so
        // the shifted symbol row has to read as its digit here. macOS and
        // Windows keep the digit and set the modifier.
        let (digit, shift) = match shifted_digit(key) {
            Some(n) => (Some(n), true),
            None => (
                key.chars()
                    .next()
                    .filter(|_| key.len() == 1)
                    .and_then(|c| c.to_digit(10)),
                mods.shift,
            ),
        };
        if let Some(n) = digit {
            if (1..=ROW_CAP as u32).contains(&n) {
                let i = n as usize - 1;
                if shift {
                    // Shift collects the row into the box for a list.
                    if let Some(value) = self.suggestions.get(i).map(|s| s.genre.clone()) {
                        self.add_to_box(&value, window, cx);
                    }
                    return;
                }
                // Ctrl with the digit is the album override, whatever the
                // switch says; the plain digit follows the switch.
                let album = mods.control || self.album_too;
                self.pick(i, album, window, cx);
            }
            return;
        }
        if mods.control {
            return;
        }
        // Shift with a digit fills the box without moving focus into it,
        // so Enter has to work from the root as well as from the box.
        if key == "enter" {
            let typed = self.typed.clone();
            let album = self.album_too;
            self.apply(typed, album, window, cx);
            return;
        }
        if key == "l" {
            self.look_up(window, cx);
            return;
        }
        if matches!(key, "right" | "s") {
            self.skip(window, cx);
        }
    }

    /// What sits at the section's right: the count, and the queue's
    /// controls. Watching, that's Begin queue; in the queue, the transport
    /// nudges and Stop queue. The nudges are the EQ window's strip without
    /// the die, since a random draw would swap the track out from under
    /// the question.
    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let total = self.items.len() as u64;
        let count = div()
            .text_xs()
            .text_color(palette::text_muted())
            .child(match self.mode {
                Mode::Queue if !self.items.is_empty() => rox_i18n::t!(
                    "tag-genres-progress",
                    at = (self.pos + 1) as u64,
                    total = total
                ),
                _ => rox_i18n::t!("tag-genres-untagged-count", count = total),
            });
        let row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_MD)
            .child(count);
        match self.mode {
            Mode::Watching => row.child(small_button(
                rox_i18n::t!("tag-genres-begin"),
                icons::PLAY,
                self.items.is_empty() || self.applying,
                cx.listener(|this, _, window, cx| this.begin_queue(window, cx)),
            )),
            Mode::Queue => row
                .child(panel::transport_nudges(&self.state.player.clone(), cx))
                .child(small_button(
                    rox_i18n::t!("tag-genres-stop"),
                    icons::STOP,
                    self.applying,
                    cx.listener(|this, _, window, cx| this.stop_queue(window, cx)),
                )),
        }
        .into_any_element()
    }

    /// The subject's card: cover, the three names, and the genre it holds
    /// now, which is the thing a retag is replacing.
    fn track_card(&self, cx: &mut Context<Self>) -> Div {
        let (Some(projection), Some(item)) = (self.projection.as_ref(), self.subject) else {
            return div();
        };
        let view = projection.resolve(item.row);
        let title: SharedString = view.title.to_string().into();
        let artist: SharedString = view.artist.to_string().into();
        let album: SharedString = view.album.to_string().into();
        let duration = fmt_ms(view.duration_ms);
        let thumb = self.key.as_ref().map(|key| {
            let path = key.path.clone();
            self.state
                .thumbs
                .update(cx, |thumbs, cx| thumbs.get(&path, cx))
        });
        let genre = if self.before.is_empty() {
            rox_i18n::t!("tag-genres-no-genre")
        } else {
            rox_i18n::t!("tag-genres-current-genre", genre = self.before.clone())
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_MD)
            .child(cover_tile(thumb))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .truncate()
                            .text_color(palette::text_bright())
                            .child(title),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(artist),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(palette::text_faint())
                            .child(if album.is_empty() {
                                duration.clone().into()
                            } else {
                                SharedString::from(format!("{album} - {duration}"))
                            }),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(if self.before.is_empty() {
                                palette::tone_warn()
                            } else {
                                palette::text_muted()
                            })
                            .child(genre),
                    ),
            )
    }

    /// The Last.fm button with its answer beside it. Above the ranking,
    /// since what it finds lands in the ranking.
    fn lookup_row(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(small_button(
                rox_i18n::t!("tag-genres-lookup"),
                icons::GLOBE,
                self.looking_up || self.applying,
                cx.listener(|this, _, window, cx| this.look_up(window, cx)),
            ))
            .child(
                div()
                    .id("genre-auto-lookup")
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_XS)
                    .text_xs()
                    .text_color(palette::text_muted())
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.auto_lookup = !this.auto_lookup;
                        // Switching it on mid-track asks for this one too,
                        // rather than only from the next seat.
                        if this.auto_lookup && this.lookup.is_empty() {
                            this.look_up(window, cx);
                        }
                        cx.notify();
                    }))
                    .child(checkbox(self.auto_lookup))
                    .child(rox_i18n::t!("tag-genres-auto-lookup")),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(palette::text_faint())
                    .map(|d| {
                        if self.looking_up {
                            d.child(rox_i18n::t!("tag-genres-looking-up"))
                        } else if let Some(note) = self.lookup_note.clone() {
                            d.child(note)
                        } else {
                            d
                        }
                    }),
            )
    }

    /// The ranking's column heads, so the rows read as what they are: a
    /// ranked list of answers, not the album's tracks.
    fn table_head(&self) -> Div {
        let head = |label: SharedString| {
            div()
                .text_xs()
                .text_color(palette::text_faint())
                .child(label)
        };
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_SM)
            .pb(px(4.))
            .border_b_1()
            .border_color(palette::border())
            .child(div().flex_none().w(px(16.)))
            .child(
                head(rox_i18n::t!("tag-genres-col-genre"))
                    .flex_1()
                    .min_w_0(),
            )
            .child(
                head(rox_i18n::t!("tag-genres-col-match"))
                    .flex_none()
                    .w(px(52. + BAR_W + 6.)),
            )
            .child(head(rox_i18n::t!("tag-genres-col-why")).flex_1().min_w_0())
    }

    /// The ranking, or what stands in for it: a spinner while the vote
    /// runs, a line saying nothing came back when it didn't. It takes
    /// whatever height the card and controls leave, and scrolls inside
    /// it: eight rows at the window's minimum height is more than fits,
    /// and the input has to stay reachable regardless.
    fn ranking(&self, cx: &mut Context<Self>) -> Div {
        let frame = div().flex_1().min_h_0().flex().flex_col();
        if self.loading {
            return frame.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_XS)
                    .px(tokens::SPACE_SM)
                    .py(tokens::SPACE_SM)
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(Spinner::new().with_size(Size::Small))
                    .child(rox_i18n::t!("tag-genres-thinking")),
            );
        }
        if self.suggestions.is_empty() {
            return frame.child(
                div()
                    .px(tokens::SPACE_SM)
                    .py(tokens::SPACE_SM)
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("tag-genres-no-suggestions")),
            );
        }
        let mut rows = div()
            .id("genre-ranking")
            .size_full()
            .flex()
            .flex_col()
            .gap(px(2.))
            .pt(px(2.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll);
        for (i, suggestion) in self.suggestions.iter().enumerate() {
            rows = rows.child(self.ranking_row(i, suggestion, cx));
        }
        frame.child(self.table_head()).child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(rows)
                .child(Scrollbar::vertical(&self.scroll)),
        )
    }

    /// One answer: its digit, the genre, the share of the vote it won with
    /// the bar beside the number, and in words what voted for it. Clicking
    /// the row applies it, with the album switch deciding how far it goes;
    /// a Shift-click collects it into the box instead.
    fn ranking_row(
        &self,
        i: usize,
        suggestion: &Suggestion,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let genre = suggestion.genre.clone();
        let share = (suggestion.score.clamp(0., 1.) * 100.) as f64;
        div()
            .id(("genre-row", i))
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_SM)
            .py(px(4.))
            .rounded(tokens::RADIUS)
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control_hover()))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                let genre = genre.clone();
                if event.modifiers().shift {
                    this.add_to_box(&genre, window, cx);
                    return;
                }
                let album = this.album_too;
                this.apply(genre, album, window, cx);
            }))
            .child(
                div()
                    .flex_none()
                    .w(px(16.))
                    .text_xs()
                    .text_color(palette::text_faint())
                    .child(format!("{}", i + 1)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(palette::text_bright())
                    .child(SharedString::from(suggestion.genre.clone())),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .flex_none()
                            .w(px(52.))
                            .text_xs()
                            .text_right()
                            .text_color(palette::text_muted())
                            .child(rox_i18n::format::format_percent(share)),
                    )
                    .child(meter(suggestion.score)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(palette::text_faint())
                    .child(SharedString::from(why(suggestion))),
            )
    }

    /// The album switch: whether an answer reaches the subject's siblings
    /// too. Shown whenever the album has any, with the count.
    fn album_switch(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let siblings = self.siblings();
        if siblings == 0 {
            return None;
        }
        Some(
            div()
                .id("genre-album-too")
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_XS)
                .text_xs()
                .text_color(palette::text_muted())
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.album_too = !this.album_too;
                    cx.notify();
                }))
                .child(checkbox(self.album_too))
                .child(rox_i18n::t!(
                    "tag-genres-album-too",
                    count = siblings as u64
                )),
        )
    }

    /// The footer: whatever's in the way on the left, the two moves that
    /// don't need an answer on the right.
    fn footer(&self, cx: &mut Context<Self>) -> Div {
        let can_undo = self.undo.is_some() && !self.applying;
        let can_skip = self.mode == Mode::Queue && !self.applying && self.subject.is_some();
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(tokens::SPACE_SM)
            .px(tokens::SPACE_MD)
            .py(tokens::SPACE_SM)
            .border_t_1()
            .border_color(palette::border())
            .bg(palette::bg_panel())
            .child(div().flex_1().min_w_0().text_xs().map(|d| {
                if self.applying {
                    let (done, total) = self.progress;
                    return d
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_SM)
                        .text_color(palette::text_muted())
                        .child(Spinner::new().with_size(Size::Small))
                        .child(rox_i18n::t!(
                            "tag-genres-writing",
                            done = done,
                            total = total
                        ))
                        .when(total > 1, |d| d.child(meter(done as f32 / total as f32)));
                }
                match self.error.clone() {
                    Some(error) => d.text_color(palette::tone_warn()).child(error),
                    None => d
                        .text_color(palette::text_faint())
                        .child(rox_i18n::t!("tag-genres-keys-hint")),
                }
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .gap(tokens::SPACE_SM)
                    .child(small_button(
                        rox_i18n::t!("tag-genres-undo"),
                        icons::ARROW_LEFT,
                        !can_undo,
                        cx.listener(|this, _, window, cx| this.undo_last(window, cx)),
                    ))
                    .child(small_button(
                        rox_i18n::t!("tag-genres-skip"),
                        icons::SKIP_FORWARD,
                        !can_skip,
                        cx.listener(|this, _, window, cx| this.skip(window, cx)),
                    )),
            )
    }

    /// The working body, once there's a track to work on.
    fn body(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .child(self.track_card(cx).flex_none())
            .child(self.lookup_row(cx))
            .child(self.ranking(cx))
            .children(self.album_switch(cx))
            .child(self.answer_row(cx))
    }

    /// The box and its Apply button. Enter in the box does the same; the
    /// button is for the hand that's on the mouse, and it's disabled while
    /// the box is empty so it can't write nothing.
    fn answer_row(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Input::new(&self.input).small()),
            )
            .child(small_button(
                rox_i18n::t!("tag-genres-apply"),
                icons::CHECK,
                self.typed.is_empty() || self.applying,
                cx.listener(|this, _, window, cx| {
                    let typed = this.typed.clone();
                    let album = this.album_too;
                    this.apply(typed, album, window, cx);
                }),
            ))
    }

    /// The page when there's nothing to ask about: the library still
    /// loading, nothing playing while watching, or a queue that ran out.
    fn empty(&self) -> Div {
        let message = if self.projection.is_none() {
            rox_i18n::t!("tag-genres-library-loading")
        } else if self.mode == Mode::Queue || self.items.is_empty() {
            rox_i18n::t!("tag-genres-empty")
        } else {
            rox_i18n::t!("tag-genres-idle")
        };
        div()
            .flex_1()
            .flex()
            .items_center()
            .justify_center()
            .px(tokens::SPACE_MD)
            .text_color(palette::text_muted())
            .child(div().max_w(px(420.)).text_center().child(message))
    }
}

impl Drop for GenreTagger {
    /// Closing the window stops the sweep. The write task outlives this
    /// entity by design, so that the files it already wrote still reach the
    /// catalog, but it has no business touching a file the user hasn't
    /// looked at.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The digit under a shifted number-row symbol, US layout. gpui on Linux
/// resolves Shift+1 to "!" and drops the modifier, and doesn't expose the
/// physical key, so the symbol is all there is to go on.
fn shifted_digit(key: &str) -> Option<u32> {
    let n = match key {
        "!" => 1,
        "@" => 2,
        "#" => 3,
        "$" => 4,
        "%" => 5,
        "^" => 6,
        "&" => 7,
        "*" => 8,
        "(" => 9,
        ")" => 0,
        _ => return None,
    };
    Some(n)
}

/// The walk entry for one projection row.
fn pending_at(projection: &Projection, row: u32) -> Pending {
    let i = row as usize;
    let album = projection.album[i];
    let named = !projection.albums.strings[album as usize].is_empty();
    Pending {
        id: projection.db_id[i],
        row,
        album: named.then_some(album),
        folder: projection.folder[i],
        sub: projection.sub[i],
    }
}

/// Every live row with no genre, in projection order, carrying what the
/// album switch groups on.
fn build_queue(projection: &Projection) -> Vec<Pending> {
    genre_suggest::untagged(projection)
        .into_iter()
        .map(|row| pending_at(projection, row))
        .collect()
}

/// Which entries of the walk share an album with the one at `at`: the same
/// album name and the same folder, so the "Greatest Hits" two different
/// artists both released stay two albums. Includes `at` itself, and is one
/// entry long for a row with no album name.
fn album_peers(items: &[Pending], at: usize) -> Vec<usize> {
    let Some(here) = items.get(at) else {
        return Vec::new();
    };
    let Some(album) = here.album else {
        return vec![at];
    };
    items
        .iter()
        .enumerate()
        .filter(|(_, p)| p.album == Some(album) && p.folder == here.folder)
        .map(|(i, _)| i)
        .collect()
}

/// Every live row of `subject`'s album in the projection, tagged or not,
/// by the same rule as [`album_peers`]. The subject alone when it has no
/// album name.
fn album_rows(projection: &Projection, subject: Pending) -> Vec<u32> {
    let Some(album) = subject.album else {
        return vec![subject.row];
    };
    (0..projection.len() as u32)
        .filter(|&row| {
            let i = row as usize;
            !projection.is_dead(row)
                && projection.album[i] == album
                && projection.folder[i] == subject.folder
        })
        .collect()
}

/// Drop the written rows out of the walk and say where it resumes: the
/// first row still standing at or after the earliest one written.
///
/// By id and projection row, never by index. A write makes the catalog fire,
/// the catalog makes the window rebuild its walk, and a rebuild replaces both
/// the list and the cursor, so any slot number taken before the write is a
/// guess by the time the write lands.
fn after_apply(items: &mut Vec<Pending>, applied: &[Pending]) -> usize {
    let gone: HashSet<i64> = applied.iter().map(|p| p.id).collect();
    let from = applied.iter().map(|p| p.row).min().unwrap_or(0);
    items.retain(|p| !gone.contains(&p.id));
    let last = items.len().saturating_sub(1);
    items
        .iter()
        .position(|p| p.row >= from)
        .unwrap_or(last)
        .min(last)
}

/// Put undone rows back into the walk where projection order says they go,
/// and say where the cursor lands: the first of them. A row the walk already
/// holds (a rebuild beat the fold-back to it) is left alone rather than
/// doubled.
fn reinsert(items: &mut Vec<Pending>, entries: &[Pending]) -> usize {
    let mut sorted: Vec<Pending> = entries.to_vec();
    sorted.sort_by_key(|p| p.row);
    for pending in &sorted {
        if items.iter().any(|p| p.id == pending.id) {
            continue;
        }
        let at = items.partition_point(|p| p.row < pending.row);
        items.insert(at, *pending);
    }
    sorted
        .first()
        .and_then(|first| items.iter().position(|p| p.id == first.id))
        .unwrap_or(0)
}

/// A suggestion's share as a bar. Sized in plain px: the bar is a glyph
/// beside a number, not a layout element the tokens have a measure for.
fn meter(score: f32) -> Div {
    let fraction = score.clamp(0., 1.);
    div()
        .flex_none()
        .w(px(BAR_W))
        .h(px(BAR_H))
        .rounded(px(BAR_H / 2.))
        .bg(palette::bg_control_hover())
        .child(
            div()
                .w(px(BAR_W * fraction))
                .h(px(BAR_H))
                .rounded(px(BAR_H / 2.))
                .bg(palette::accent()),
        )
}

/// What voted for a suggestion, in words: rows on the album, rows by the
/// artist, neighbours that sound like it, and Last.fm when the lookup named
/// it. A source with nothing behind it is left out rather than shown as a
/// zero, so the line reads as a short list of reasons.
fn why(suggestion: &Suggestion) -> String {
    let mut parts: Vec<String> = Vec::new();
    if suggestion.album > 0 {
        parts.push(
            rox_i18n::t!("tag-genres-why-album", count = suggestion.album as u64).to_string(),
        );
    }
    if suggestion.artist > 0 {
        parts.push(
            rox_i18n::t!("tag-genres-why-artist", count = suggestion.artist as u64).to_string(),
        );
    }
    if suggestion.acoustic > 0 {
        parts.push(
            rox_i18n::t!(
                "tag-genres-why-acoustic",
                count = suggestion.acoustic as u64
            )
            .to_string(),
        );
    }
    if suggestion.lookup {
        parts.push(rox_i18n::t!("tag-genres-why-lookup").to_string());
    }
    parts.join(", ")
}

/// The current track's cover: the thumbnail once it's ready, a note glyph
/// while it loads or when the file has none. The duplicates window's tile,
/// scaled up.
fn cover_tile(thumb: Option<Thumb>) -> Div {
    let side = px(COVER);
    let ready = match thumb {
        Some(Thumb::Ready(image)) => Some(image),
        _ => None,
    };
    div()
        .flex_none()
        .size(side)
        .rounded(tokens::RADIUS)
        .overflow_hidden()
        .bg(palette::bg_control())
        .flex()
        .items_center()
        .justify_center()
        .map(|d| match ready {
            Some(image) => d.child(img(image).size_full().object_fit(ObjectFit::Cover)),
            None => d.child(
                svg()
                    .path(icons::MUSIC)
                    .size(px(24.))
                    .text_color(palette::text_faint()),
            ),
        })
}

impl Render for GenreTagger {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The page renders under the player's art tint like the workspace
        // that opened it, and claims the widget theme while it holds focus,
        // the health window's move. Without the wrapper the palette reads
        // untinted and the window sits grey beside its themed siblings.
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);
        panel::window_body(player, || self.page(window, cx))
    }
}

impl GenreTagger {
    /// The whole window, built inside the tint scope.
    fn page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let header = self.header(cx);
        let body = if self.subject.is_some() {
            self.body(cx).into_any_element()
        } else {
            self.empty().into_any_element()
        };
        let page = div()
            .id("genre-tagger-page")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .p(tokens::SPACE_MD)
            .bg(palette::bg_elevated())
            .child(
                section(rox_i18n::t!("tag-genres-heading"), Some(header), body)
                    .flex_1()
                    .min_h_0(),
            );

        div()
            .size_full()
            .track_focus(&self.focus)
            .flex()
            .flex_row()
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_key(event, window, cx);
            }))
            .children(self.backdrop.layer(&self.state.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(page)
                    .child(self.footer(cx)),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(id: i64, album: Option<u32>, folder: u32) -> Pending {
        Pending {
            id,
            row: id as u32,
            album,
            folder,
            sub: 0,
        }
    }

    #[test]
    fn album_peers_group_on_name_and_folder() {
        let items = vec![
            pending(1, Some(7), 1),
            pending(2, Some(7), 1),
            pending(3, Some(7), 2),
            pending(4, Some(8), 1),
            pending(5, None, 1),
            pending(6, None, 1),
        ];
        assert_eq!(album_peers(&items, 0), vec![0, 1]);
        assert_eq!(album_peers(&items, 2), vec![2]);
        assert_eq!(album_peers(&items, 4), vec![4]);
        assert_eq!(album_peers(&items, 9), Vec::<usize>::new());
    }

    #[test]
    fn after_apply_lands_on_the_next_survivor() {
        let mut items = (1..=5).map(|i| pending(i, None, 0)).collect::<Vec<_>>();
        let pos = after_apply(&mut items, &[pending(2, None, 0)]);
        assert_eq!(pos, 1);
        assert_eq!(items[pos].id, 3);
    }

    #[test]
    fn after_apply_ignores_where_the_cursor_was() {
        let mut items = (1..=5).map(|i| pending(i, None, 0)).collect::<Vec<_>>();
        let pos = after_apply(&mut items, &[pending(1, None, 0), pending(4, None, 0)]);
        assert_eq!(pos, 0);
        assert_eq!(items[pos].id, 2);
    }

    #[test]
    fn after_apply_survives_a_rebuild_that_already_dropped_the_rows() {
        // The catalog fired between the write and the fold-back, so the
        // walk is already short and every slot the write remembered is off
        // by one. Ids still name the right rows.
        let mut items = vec![
            pending(1, None, 0),
            pending(4, None, 0),
            pending(5, None, 0),
        ];
        let pos = after_apply(&mut items, &[pending(2, None, 0), pending(3, None, 0)]);
        let ids: Vec<i64> = items.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![1, 4, 5]);
        assert_eq!(items[pos].id, 4);
    }

    #[test]
    fn after_apply_clamps_at_the_end() {
        let mut items = (1..=3).map(|i| pending(i, None, 0)).collect::<Vec<_>>();
        let pos = after_apply(&mut items, &[pending(3, None, 0)]);
        assert_eq!(pos, 1);
        let pos = after_apply(&mut items, &[pending(1, None, 0), pending(2, None, 0)]);
        assert_eq!(pos, 0);
        assert!(items.is_empty());
    }

    #[test]
    fn reinsert_restores_projection_order_and_the_cursor() {
        let mut items = vec![pending(1, None, 0), pending(4, None, 0)];
        let pos = reinsert(&mut items, &[pending(3, None, 0), pending(2, None, 0)]);
        assert_eq!(pos, 1);
        assert_eq!(items[pos].id, 2);
        let ids: Vec<i64> = items.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![1, 2, 3, 4]);
    }

    #[test]
    fn reinsert_leaves_a_row_the_walk_already_holds() {
        let mut items = vec![pending(1, None, 0), pending(2, None, 0)];
        let pos = reinsert(&mut items, &[pending(2, None, 0)]);
        assert_eq!(pos, 1);
        let ids: Vec<i64> = items.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![1, 2]);
    }
}
