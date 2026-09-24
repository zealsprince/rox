//! The genre tagger: one track at a time, playing while it asks. Genre is
//! the field a scanner can't infer, so this is a listening job: rank the
//! likely answers with their evidence, take a typed one for the rest, write,
//! move on. Nothing batches silently and nothing guesses for the user.
//! Shift with a digit or a Shift-click collects genres into the box for a
//! track that's more than one.
//!
//! Opened cold it watches the player and never touches the transport. "Begin
//! queue" walks every untagged track instead, playing each in turn.
//!
//! Suggestions come from [`rox_library::genre_suggest`]'s vote over the
//! album, the artist, and acoustic neighbours, plus Last.fm's artist tags
//! when looked up. Writes go through the atomic tag layer (ADR 4); a cue
//! subsong is refused, since nothing inside a shared image means "track 4".
//! The album switch reaches rows sharing an album name and a folder. One
//! level of undo restores every file the last write touched.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    AnyElement, App, AsyncApp, Bounds, ClickEvent, Context, Div, Entity, FocusHandle, Global,
    KeyDownEvent, ObjectFit, ScrollHandle, SharedString, Stateful, Subscription, Window,
    WindowHandle, div, img, prelude::*, px, size, svg,
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
use rox_panel_kit::ui::{MIN_SIZE, checkbox, section, small_button};
use rox_services::backdrop::WindowBackdrop;
use rox_services::catalog::LibraryEvent;
use rox_services::thumbs::Thumb;

/// Eight because the digit row picks them.
const ROW_CAP: usize = 8;

const COVER: f32 = 88.;

/// A third of the way in skips the intro, which says least about genre.
const START_DIVISOR: u32 = 3;

/// Caps the offset so a long mix doesn't start minutes in.
const START_CAP_MS: u32 = 30_000;

const BAR_W: f32 = 56.;
const BAR_H: f32 = 3.;

/// Past the first few, Last.fm's tags turn into "seen live" and "favourites".
const LOOKUP_TAGS: usize = 5;

/// One at a time: a walk with a write in flight isn't worth losing to a second copy.
#[derive(Default)]
struct OpenTagger(Option<WindowHandle<Root>>);

impl Global for OpenTagger {}

pub fn open(state: AppState, cx: &mut App) {
    if let Some(handle) = cx.try_global::<OpenTagger>().and_then(|o| o.0)
        && handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
    {
        return;
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

/// `id` survives a projection swap; `row` is in the walk's projection.
/// `album` is None for a row with no album name, so loose singles never
/// group into one album.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pending {
    id: i64,
    row: u32,
    album: Option<u32>,
    folder: u32,
    /// Non-zero is a cue track, which has nowhere on disk to write to.
    sub: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// Retag whatever plays; the transport stays the user's.
    Watching,
    Queue,
}

/// Enough to take a write back exactly. `walk` marks a row the write took out
/// of the untagged walk, which undo puts back.
#[derive(Clone, Debug)]
struct Written {
    walk: bool,
    pending: Pending,
    key: TrackKey,
    before: String,
}

pub struct GenreTagger {
    state: AppState,
    /// Held whole so a swap mid-vote can't shift rows under a result on its way back.
    projection: Option<Arc<Projection>>,
    mode: Mode,
    items: Vec<Pending>,
    pos: usize,
    /// The row at the cursor in the queue, or the playing track while watching.
    subject: Option<Pending>,
    key: Option<TrackKey>,
    before: String,
    suggestions: Vec<Suggestion>,
    loading: bool,
    /// Bumped on every seat so a stale batch of suggestions drops.
    generation: u64,
    input: Entity<InputState>,
    typed: String,
    error: Option<SharedString>,
    applying: bool,
    /// (done, total) for an album sweep's footer count.
    progress: (usize, usize),
    /// Raised on close; the sweep checks it between files.
    cancel: Arc<AtomicBool>,
    undo: Option<Vec<Written>>,
    /// Sticky across tracks; Ctrl with a digit overrides it per pick.
    album_too: bool,
    /// The Last.fm tags fed to the vote. Empty until a lookup answers.
    lookup: Vec<String>,
    looking_up: bool,
    auto_lookup: bool,
    lookup_note: Option<SharedString>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    backdrop: WindowBackdrop,
    _backdrop_changed: Subscription,
    _library_changed: Subscription,
    /// Also moves the subject onto whatever starts playing while watching.
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

    /// Rebuild the walk, keeping the cursor by database id. A row this window just
    /// tagged is gone, so the index stays and lands on what slid into its place.
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
                // Re-seat only when the cursor moved: the catalog fires on every rating and
                // scrobble, and restarting the track each time would make the walk unusable.
                let moved = first || holding != self.items.get(pos).map(|p| p.id);
                if moved {
                    self.seat(window, cx);
                } else {
                    cx.notify();
                }
            }
            Mode::Watching => {
                // Refresh the card without re-asking the vote or clearing the box.
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

    /// None for a stream or a file the library hasn't indexed.
    fn playing_subject(&self, cx: &App) -> Option<Pending> {
        let projection = self.projection.as_ref()?;
        let key = self.state.player.read(cx).now_playing()?.key;
        let id = self.state.library.read(cx).id_for_key(&key)?;
        let row = projection.db_id.iter().position(|&db| db == id)? as u32;
        Some(pending_at(projection, row))
    }

    fn genre_of(&self, subject: Option<Pending>) -> String {
        match (self.projection.as_ref(), subject) {
            (Some(projection), Some(p)) => projection.resolve(p.row).genre.to_string(),
            _ => String::new(),
        }
    }

    /// Compared by id so position ticks don't re-seat the same track.
    fn follow_player(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode != Mode::Watching {
            return;
        }
        let subject = self.playing_subject(cx);
        if subject.map(|p| p.id) != self.subject.map(|p| p.id) {
            self.seat(window, cx);
        }
    }

    /// The album switch and focus are left alone: they belong to the user, not the track.
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

    /// Resumes from the cursor rather than the top.
    fn begin_queue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        self.mode = Mode::Queue;
        self.pos = self.pos.min(self.items.len() - 1);
        self.seat(window, cx);
    }

    fn stop_queue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = Mode::Watching;
        self.seat(window, cx);
    }

    /// Spliced in after the playing track like Play Now, so the pass doesn't eat
    /// the queue (ADR 16). The offset rides the insert, so the head is never heard.
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
            player.play_now_at(key, start as f64 / 1000.0, cx);
        });
    }

    /// A connection per request: cheap beside the nearest-neighbour query.
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

    /// Artist-level, the read Last.fm offers without an account. Honours the
    /// provider switch, and says so when it's off.
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
                        // Last.fm's tags are lowercase; the library capitalizes.
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

    fn skip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mode != Mode::Queue || self.applying || self.items.is_empty() {
            return;
        }
        self.pos = (self.pos + 1).min(self.items.len() - 1);
        self.seat(window, cx);
    }

    fn pick(&mut self, n: usize, album: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(genre) = self.suggestions.get(n).map(|s| s.genre.clone()) else {
            return;
        };
        self.apply(genre, album, window, cx);
    }

    /// Joined in the library's "; " spelling, never doubled.
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

    /// With `album`: in the queue, the album's other untagged rows; watching, the
    /// whole album.
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
        // A cue row drops out of an album sweep rather than stopping it.
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

    /// Write `genre` to every job and fold the results back in. A None `genre`
    /// is an undo: each file gets its own `before` back.
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
        // The library outlives the window, and must hear about files already written.
        let library = self.state.library.clone();
        cx.spawn_in(window, async move |this, cx| {
            let mut written: Vec<(Written, Change)> = Vec::new();
            let mut failure: Option<SharedString> = None;
            for job in jobs {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
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
            // The library first, through the app: files are already changed on disk,
            // and a closed window must not keep the database from hearing. One batch,
            // one reindex: per-file folds would hit the catalog's busy gate.
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
            this.update_in(cx, |this, window, cx| {
                this.applying = false;
                this.error = failure;
                if written.is_empty() {
                    cx.notify();
                    return;
                }
                if taking_back {
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
                    // A retag also takes its rows out of the walk, so Begin queue skips them.
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

    /// Handled on the root, not the keymap: binding digits app-wide would be
    /// absurd. All but Escape need the root focused, so the box still types.
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
        // On Linux gpui reports Shift+1 as "!" with shift cleared.
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
                // Ctrl forces the album; the plain digit follows the switch.
                let album = mods.control || self.album_too;
                self.pick(i, album, window, cx);
            }
            return;
        }
        if mods.control {
            return;
        }
        // Shift+digit fills the box without focusing it, so Enter works from the root too.
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

    /// The queue's nudges leave out the die: a random draw would swap the track mid-question.
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

    /// Scrolls inside the space left, so the input stays reachable at minimum height.
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
    /// The write task outlives this entity so written files reach the catalog,
    /// but it stops before touching another file.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// US layout. gpui on Linux drops the modifier and doesn't expose the
/// physical key, so the symbol is all there is.
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

fn build_queue(projection: &Projection) -> Vec<Pending> {
    genre_suggest::untagged(projection)
        .into_iter()
        .map(|row| pending_at(projection, row))
        .collect()
}

/// Same album name and folder, so two artists' "Greatest Hits" stay apart.
/// Includes `at`.
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

/// Tagged or not, by the rule [`album_peers`] uses.
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

/// Resumes at the first row at or after the earliest written one. By id and
/// row, never index: the write triggers a rebuild that replaces the list.
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

/// Returns the cursor on the first of them. Rows the walk already holds stay single.
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

/// Sources with nothing behind them are left out rather than shown as zero.
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
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);
        panel::window_body(player, || self.page(window, cx))
    }
}

impl GenreTagger {
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
        // A rebuild already dropped the rows, so remembered slots are off by one.
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
