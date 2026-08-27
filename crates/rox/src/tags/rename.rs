//! Renaming files from their tags: foobar2000's file operations, the
//! guesser run backwards. A pattern like `%albumartist%/%album%/%track% -
//! %title%` renders each selected track's tags into a path under the
//! library root that track is already under, keeps the file's own
//! extension, and shows every move before any of them happen. Apply moves
//! the files and moves the rows with [`Library::rename_files`], so ids,
//! ratings, play counts, and playlist membership all persist across the move.
//!
//! The values come off the catalog's projection rather than a fresh read
//! of every file, so the preview updates as fast as you type. That leaves
//! %comment% with nothing to render (the projection doesn't include it) and
//! it falls back like any missing field.
//!
//! What this refuses rather than guesses: a cue track, which is a span
//! inside an image the whole disc shares and has no file of its own to
//! move; a track outside every library root, which has no root to render
//! under; a track the projection has no row for, which has no tag values
//! to render from; and any move whose destination already exists or that
//! two tracks both target. A shuffle inside the selection (track 2 taking
//! track 1's name) reads as occupied and refuses too, rather than ordering
//! itself into a sequence that half-finishes if it fails.
//!
//! Thumbnails and waveform peaks are keyed by path, so a moved file loses
//! its cached ones and regenerates them on next sight. Lyrics sidecars
//! travel with the file.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use gpui::{
    actions, div, prelude::*, px, size, App, Bounds, Context, Div, Entity, Focusable as _, Global,
    KeyBinding, ScrollHandle, SharedString, Subscription, Window, WindowHandle,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::Scrollbar;
use gpui_component::spinner::Spinner;
use gpui_component::{Root, Sizable, Size};

use rox_core::settings::Settings;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_library::lyrics;
use rox_library::writer::{Field, CLONE_SUFFIX};
use rox_panel_api::panel::AppState;
use rox_panel_kit::ui::{self as settings_ui, kbd_line, section, small_button, Seg};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::catalog::Library;

use crate::matching::{open_or_focus, WindowRegistry};
use crate::tags::guess;

/// The pattern a first run starts on: the layout most libraries already
/// half-follow, so the preview shows mostly no-ops instead of chaos.
const DEFAULT_PATTERN: &str = "%albumartist%/%album%/%track% - %title%";

/// How many patterns the dialog remembers. Enough for the two or three
/// schemes a library actually uses, short enough to stay a row of chips.
const REMEMBERED: usize = 6;

actions!(rename, [Apply]);

/// The key context the window's own bindings scope to.
const CONTEXT: &str = "RenameFiles";

/// The dialog's apply binding; call once at startup. It's on the
/// window root, so enter applies wherever focus is and not only in the
/// pattern field. The input still sees the key first, since its own
/// binding is deeper along the focus path and it propagates up to here;
/// the guard at the top of [`RenameFiles::apply`] eats the second arrival.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("enter", Apply, Some(CONTEXT))]);
}

/// The open rename dialogs, keyed by their selection.
#[derive(Default)]
struct OpenRenamers(Vec<(Vec<i64>, WindowHandle<Root>)>);

impl Global for OpenRenamers {}

impl WindowRegistry for OpenRenamers {
    type Key = Vec<i64>;
    fn entries(&mut self) -> &mut Vec<(Vec<i64>, WindowHandle<Root>)> {
        &mut self.0
    }
}

/// Open the rename dialog on `ids`, or bring the one already on that
/// selection to the front. An empty selection opens nothing.
pub fn open(state: AppState, ids: Vec<i64>, cx: &mut App) {
    if ids.is_empty() {
        return;
    }
    let mut key = ids.clone();
    key.sort_unstable();
    open_or_focus::<OpenRenamers>(
        key,
        move |cx| {
            let (width, height) = Settings::load()
                .windows
                .rename_dialog
                .filter(|s| s.width >= 400. && s.height >= 300.)
                .map(|s| (s.width, s.height))
                .unwrap_or((1000., 620.));
            let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
            rox_panel_api::panel::open_child_window(
                cx,
                rox_i18n::t!("tags-rename-window-title"),
                bounds,
                Some(settings_ui::MIN_SIZE),
                move |window, cx| cx.new(|cx| RenameFiles::new(state, ids, window, cx)),
            )
        },
        cx,
    );
}

/// One selected track as the plan reads it: where its file is, which
/// root it's under, and the tag values the pattern renders from.
struct Track {
    from: PathBuf,
    /// Which subsong of its file the row is, 0 for a plain file.
    sub: u16,
    /// The library root the file is under, None when it's under none of
    /// them.
    root: Option<PathBuf>,
    /// The tag values the pattern renders from, None when the catalog has
    /// no projection row for the track. Rendering an unresolved row would
    /// take every field's fallback and file the track under "Unknown
    /// Artist", so it doesn't render at all.
    values: Option<Vec<(Field, String)>>,
}

/// Why a track stays where it is.
#[derive(Clone, Debug, PartialEq)]
enum Blocked {
    /// A cue track is a span inside an image the whole disc shares.
    /// Moving it would move every other track of that rip, and the sheet
    /// pointing at the image would go stale, so a cue track never moves.
    /// The same rule the tag writer keeps in `writer::writes_to_file`.
    CueTrack,
    /// The file is under no library root, so there's no folder to
    /// render the pattern under.
    OutsideRoots,
    /// The catalog hasn't got a projection row for this track, which is
    /// where the tag values come from. Nothing to render against, and
    /// rendering the fallbacks anyway would name it "Unknown Artist".
    Unresolved,
    /// The pattern can't render this track's values at all.
    Render(String),
    /// Another selected track renders the same destination.
    Duplicate,
    /// Something is already at the destination.
    Occupied,
}

impl Blocked {
    fn label(&self) -> SharedString {
        match self {
            Blocked::CueTrack => rox_i18n::t!("tags-rename-blocked-cue"),
            Blocked::OutsideRoots => rox_i18n::t!("tags-rename-blocked-outside-roots"),
            Blocked::Unresolved => rox_i18n::t!("tags-rename-blocked-unresolved"),
            Blocked::Render(e) => e.clone().into(),
            Blocked::Duplicate => rox_i18n::t!("tags-rename-blocked-duplicate"),
            Blocked::Occupied => rox_i18n::t!("tags-rename-blocked-occupied"),
        }
    }
}

/// One row of the plan: where the file is and where the pattern puts it.
struct Move {
    from: PathBuf,
    to: PathBuf,
    /// The move only changes the path's casing, which on a
    /// case-insensitive filesystem is a rename onto itself. It goes
    /// through a temp name so the filesystem sees two distinct steps.
    case_only: bool,
    /// The destination is exactly where the file already is.
    unchanged: bool,
    blocked: Option<Blocked>,
}

impl Move {
    /// Whether this row moves a file when Apply runs.
    fn moves(&self) -> bool {
        self.blocked.is_none() && !self.unchanged
    }
}

/// Append `ext` to a rendered path. `set_extension` would eat everything
/// after the last dot of the rendered name, which a title like "R.E.M."
/// or "Vol. 2" leaves plenty of.
fn with_extension(path: PathBuf, ext: Option<&std::ffi::OsStr>) -> PathBuf {
    let Some(ext) = ext else { return path };
    let mut name: OsString = path.as_os_str().to_os_string();
    name.push(".");
    name.push(ext);
    PathBuf::from(name)
}

/// Whether two paths differ only in casing, the case-only rename a
/// case-insensitive filesystem reads as a no-op.
fn same_but_case(a: &Path, b: &Path) -> bool {
    a != b && a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

/// Render every track through `pattern` and sort out what can actually
/// move. `exists` reports whether a path is taken, injected so the plan
/// can be tested without a filesystem.
fn plan(tracks: &[Track], pattern: &guess::Pattern, exists: &dyn Fn(&Path) -> bool) -> Vec<Move> {
    let mut moves: Vec<Move> = Vec::with_capacity(tracks.len());
    for track in tracks {
        let blocked = if track.sub != 0 {
            Some(Blocked::CueTrack)
        } else if track.root.is_none() {
            Some(Blocked::OutsideRoots)
        } else if track.values.is_none() {
            Some(Blocked::Unresolved)
        } else {
            None
        };
        if let Some(blocked) = blocked {
            moves.push(Move {
                from: track.from.clone(),
                to: track.from.clone(),
                case_only: false,
                unchanged: false,
                blocked: Some(blocked),
            });
            continue;
        }
        let root = track.root.clone().unwrap_or_default();
        let values = track.values.as_deref().unwrap_or_default();
        let to = match pattern.render(values) {
            Ok(rendered) => with_extension(root.join(rendered), track.from.extension()),
            Err(e) => {
                moves.push(Move {
                    from: track.from.clone(),
                    to: track.from.clone(),
                    case_only: false,
                    unchanged: false,
                    blocked: Some(Blocked::Render(e)),
                });
                continue;
            }
        };
        let unchanged = to == track.from;
        let case_only = same_but_case(&track.from, &to);
        moves.push(Move {
            from: track.from.clone(),
            to,
            case_only,
            unchanged,
            blocked: None,
        });
    }
    // Two sources onto one destination: neither is safe, since whichever
    // moves second overwrites the first. Both rows say so rather than one
    // silently winning.
    let mut seen: HashMap<PathBuf, usize> = HashMap::new();
    for mv in moves.iter().filter(|mv| mv.moves()) {
        *seen.entry(mv.to.clone()).or_default() += 1;
    }
    for mv in moves.iter_mut() {
        if mv.moves() && seen.get(&mv.to).copied().unwrap_or(0) > 1 {
            mv.blocked = Some(Blocked::Duplicate);
        }
    }
    // A destination that already exists on disk. The file's own path is
    // not a collision, and neither is the case-only variant of it, which
    // is the same file on a case-insensitive filesystem.
    for mv in moves.iter_mut() {
        if mv.moves() && !mv.case_only && exists(&mv.to) {
            mv.blocked = Some(Blocked::Occupied);
        }
    }
    moves
}

/// The writer's clone naming for the intermediate step of a move, so the
/// watcher's clone filter ignores it the way it ignores a tag write's.
fn hop_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(CLONE_SUFFIX);
    path.with_file_name(name)
}

/// Whether a failed rename failed because the two paths are on different
/// filesystems, the one error a copy can get past.
fn cross_device(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    let code = 18; // EXDEV
    #[cfg(windows)]
    let code = 17; // ERROR_NOT_SAME_DEVICE
    #[cfg(not(any(unix, windows)))]
    let code = -1;
    e.raw_os_error() == Some(code)
}

/// Move one file to `to`, making the folders above it first. A plain
/// rename does it inside one filesystem. Across two it can't, so the
/// bytes get copied to a clone beside the destination, flushed, renamed
/// into place, and only then is the original unlinked: an interrupted
/// copy costs a stray clone the watcher already ignores, never the file.
/// A case-only rename hops through the same clone name, since asking a
/// case-insensitive filesystem to rename a file onto itself does nothing.
fn move_file(from: &Path, to: &Path, case_only: bool) -> Result<(), String> {
    if let Some(dir) = to.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    if case_only {
        let hop = hop_path(from);
        fs::rename(from, &hop).map_err(|e| format!("{e}"))?;
        return fs::rename(&hop, to).map_err(|e| format!("{e}"));
    }
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(e) if cross_device(&e) => {
            let hop = hop_path(to);
            let copy = (|| -> std::io::Result<()> {
                fs::copy(from, &hop)?;
                fs::File::open(&hop)?.sync_all()?;
                fs::rename(&hop, to)
            })();
            if let Err(e) = copy {
                let _ = fs::remove_file(&hop);
                return Err(format!("{e}"));
            }
            fs::remove_file(from).map_err(|e| format!("{e}"))
        }
        Err(e) => Err(format!("{e}")),
    }
}

/// Move a track's lyrics sidecars along with it. The candidate lists line
/// up position by position, so a `.lrc` beside the old name ends up beside
/// the new one in the same convention. Best effort: a sidecar that fails
/// to move leaves the audio file where it now is, which is the
/// half that matters. Returns the pairs that moved.
fn move_sidecars(from: &Path, to: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut moved = Vec::new();
    for (side_from, side_to) in lyrics::sidecar_candidates(from)
        .into_iter()
        .zip(lyrics::sidecar_candidates(to))
    {
        if !side_from.exists() || side_to.exists() {
            continue;
        }
        if move_file(&side_from, &side_to, false).is_ok() {
            moved.push((side_from, side_to));
        }
    }
    moved
}

pub struct RenameFiles {
    library: Entity<Library>,
    tracks: Vec<Track>,
    /// The pattern input. Seeded from the last applied pattern.
    pattern: Entity<InputState>,
    /// The patterns applied before, newest first, offered as chips.
    remembered: Vec<SharedString>,
    /// The current pattern's plan, rebuilt when the pattern changes
    /// rather than per frame: it stats the disk for every destination,
    /// which isn't something a repaint should pay for.
    plan: Vec<Move>,
    /// What's wrong with the pattern itself, when nothing parses.
    parse_error: Option<SharedString>,
    /// A failed move, shown inline over the buttons.
    error: Option<SharedString>,
    /// Moves are in flight; the input locks and the buttons hold still.
    applying: bool,
    done: usize,
    total: usize,
    scroll: ScrollHandle,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    _input_events: Vec<Subscription>,
    _backdrop_changed: Subscription,
}

impl RenameFiles {
    fn new(state: AppState, ids: Vec<i64>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let roots = state.library.read(cx).roots();
        let projection = state.library.read(cx).projection().cloned();
        let tracks = {
            let library = state.library.read(cx);
            let row_of: HashMap<i64, u32> = projection
                .as_ref()
                .map(|projection| {
                    projection
                        .db_id
                        .iter()
                        .enumerate()
                        .map(|(row, &id)| (id, row as u32))
                        .collect()
                })
                .unwrap_or_default();
            let mut tracks = Vec::with_capacity(ids.len());
            for &id in &ids {
                let Some(from) = library
                    .paths_for(&[id])
                    .ok()
                    .and_then(|mut paths| paths.pop())
                else {
                    continue;
                };
                // The deepest root that contains the file: roots never
                // nest, so there's at most one, and the rendered path
                // is built under it.
                let root = roots.iter().find(|r| from.starts_with(r)).cloned();
                let resolved = projection.as_ref().and_then(|projection| {
                    let row = *row_of.get(&id)?;
                    let v = projection.resolve(row);
                    let mut values = vec![
                        (Field::Title, v.title.to_owned()),
                        (Field::Artist, v.artist.to_owned()),
                        (Field::AlbumArtist, v.album_artist.to_owned()),
                        (Field::Album, v.album.to_owned()),
                        (Field::Genre, v.genre.to_owned()),
                    ];
                    // A zero is the catalog's way of saying the file has
                    // no number, so it renders as missing rather than as
                    // "00" or the year 0.
                    for (field, number) in [
                        (Field::Year, v.year),
                        (Field::TrackNo, v.track_no),
                        (Field::DiscNo, v.disc_no),
                    ] {
                        if number > 0 {
                            values.push((field, number.to_string()));
                        }
                    }
                    Some((values, v.sub))
                });
                // An unresolved row keeps its subsong at 0: with no
                // projection there's nothing to say it's a cue span, and
                // the missing values block it either way.
                let sub = resolved.as_ref().map(|(_, sub)| *sub).unwrap_or(0);
                tracks.push(Track {
                    from,
                    sub,
                    root,
                    values: resolved.map(|(values, _)| values),
                });
            }
            tracks
        };
        let saved = Settings::load().windows.rename_dialog.unwrap_or_default();
        let remembered: Vec<SharedString> = saved
            .patterns
            .iter()
            .filter(|p| !p.trim().is_empty())
            .take(REMEMBERED)
            .map(|p| SharedString::from(p.clone()))
            .collect();
        let seed = remembered
            .first()
            .map(|p| p.to_string())
            .unwrap_or_else(|| DEFAULT_PATTERN.to_owned());
        let pattern = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(seed)
                .placeholder(DEFAULT_PATTERN)
        });
        let mut _input_events = Vec::new();
        _input_events.push(cx.subscribe_in(
            &pattern,
            window,
            |this: &mut Self, _, event: &InputEvent, window, cx| match event {
                // Enter applies: the preview is the confirmation, and it
                // is right there above the button.
                InputEvent::PressEnter { .. } => this.apply(window, cx),
                InputEvent::Change => this.replan(cx),
                _ => {}
            },
        ));
        window.focus(&pattern.read(cx).focus_handle(cx));
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| this.persist_frame(window, cx));
            }
            true
        });
        let mut this = RenameFiles {
            library: state.library,
            tracks,
            pattern,
            remembered,
            plan: Vec::new(),
            parse_error: None,
            error: None,
            applying: false,
            done: 0,
            total: 0,
            scroll: ScrollHandle::new(),
            now_art: state.now_art,
            backdrop: WindowBackdrop::default(),
            _input_events,
            _backdrop_changed,
        };
        this.replan(cx);
        this
    }

    /// Rebuild the plan from the pattern as it stands. Runs on every
    /// keystroke in the pattern, so it does the disk probing the render
    /// must not.
    fn replan(&mut self, cx: &mut Context<Self>) {
        match guess::parse(self.pattern.read(cx).value().trim()) {
            Ok(pattern) => {
                self.plan = plan(&self.tracks, &pattern, &|path| path.exists());
                self.parse_error = None;
            }
            Err(e) => {
                self.plan.clear();
                self.parse_error = Some(e.into());
            }
        }
        cx.notify();
    }

    /// How many of the selection the current plan actually moves.
    fn movable(&self) -> usize {
        self.plan.iter().filter(|mv| mv.moves()).count()
    }

    /// Move the files, one background hop each, then move the rows in one
    /// batch. Each file is noted as a self-rename right before it moves,
    /// so the watcher's echo of a move rox just made matches nothing;
    /// noting the whole batch up front instead would let the suppression
    /// window expire under a long run.
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.applying {
            return;
        }
        let moves: Vec<(PathBuf, PathBuf, bool, SharedString)> = self
            .plan
            .iter()
            .filter(|mv| mv.moves())
            .map(|mv| {
                let name = mv
                    .from
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| mv.from.display().to_string());
                (mv.from.clone(), mv.to.clone(), mv.case_only, name.into())
            })
            .collect();
        if moves.is_empty() {
            return;
        }
        self.applying = true;
        self.done = 0;
        self.total = moves.len();
        self.error = None;
        self.remember(cx);
        cx.notify();
        let library = self.library.clone();
        cx.spawn_in(window, async move |this, cx| {
            let mut landed: Vec<(PathBuf, PathBuf)> = Vec::new();
            let mut failures = 0usize;
            let mut first_error: Option<SharedString> = None;
            for (from, to, case_only, name) in moves {
                if library
                    .update(cx, |library, _| {
                        library.note_self_rename([(from.clone(), to.clone())])
                    })
                    .is_err()
                {
                    return;
                }
                let (from, to, result) = cx
                    .background_executor()
                    .spawn(async move {
                        let result = move_file(&from, &to, case_only).map(|()| {
                            // Sidecars only follow a file that actually
                            // landed; moving them first would strand them
                            // beside a name with no audio.
                            move_sidecars(&from, &to)
                        });
                        (from, to, result)
                    })
                    .await;
                match result {
                    Ok(sidecars) => {
                        if library
                            .update(cx, |library, _| library.note_self_rename(sidecars))
                            .is_err()
                        {
                            return;
                        }
                        landed.push((from, to));
                    }
                    Err(e) => {
                        failures += 1;
                        if first_error.is_none() {
                            first_error = Some(rox_i18n::t!(
                                "tags-rename-move-error",
                                name = name.to_string(),
                                error = e
                            ));
                        }
                    }
                }
                if this
                    .update(cx, |this, cx| {
                        this.done += 1;
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            this.update_in(cx, move |this, window, cx| {
                if !landed.is_empty() {
                    // The rows follow the files in one batch, ids intact.
                    library.update(cx, |library, cx| library.rename_files(landed, cx));
                }
                match first_error {
                    None => {
                        this.persist_frame(window, cx);
                        window.remove_window();
                    }
                    Some(e) => {
                        // The tracks are now where the finished moves put
                        // them, so replanning makes a retry diff against
                        // the current state instead of re-moving.
                        this.applying = false;
                        this.error = Some(if failures > 1 {
                            rox_i18n::t!(
                                "tags-rename-move-errors",
                                count = failures as u64,
                                error = e.to_string()
                            )
                        } else {
                            e
                        });
                        this.reseat();
                        this.replan(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Point every track at the file it now has, after a partial apply.
    /// A landed move left the file at the destination the plan named, so
    /// that destination is where the next plan starts from.
    fn reseat(&mut self) {
        let landed: HashMap<PathBuf, PathBuf> = self
            .plan
            .iter()
            .filter(|mv| mv.moves() && mv.to.exists() && !mv.from.exists())
            .map(|mv| (mv.from.clone(), mv.to.clone()))
            .collect();
        for track in &mut self.tracks {
            if let Some(to) = landed.get(&track.from) {
                track.from = to.clone();
            }
        }
    }

    /// Put the applied pattern at the head of the remembered list.
    fn remember(&mut self, cx: &App) {
        let pattern = self.pattern.read(cx).value().trim().to_owned();
        if pattern.is_empty() {
            return;
        }
        self.remembered
            .retain(|p| p.as_ref() != pattern.as_str() && !p.trim().is_empty());
        self.remembered.insert(0, pattern.into());
        self.remembered.truncate(REMEMBERED);
    }

    /// Write the window frame and the remembered patterns into the
    /// settings file, the restore for the next dialog.
    fn persist_frame(&self, window: &Window, _cx: &App) {
        let frame = window.window_bounds().get_bounds();
        let patterns: Vec<String> = self.remembered.iter().map(|p| p.to_string()).collect();
        Settings::update(move |s| {
            let state = s.windows.rename_dialog.get_or_insert_with(Default::default);
            state.width = frame.size.width.into();
            state.height = frame.size.height.into();
            state.patterns = patterns;
        });
    }

    /// One preview row: where the file is now over where it would go,
    /// both relative to the library root so the pattern's own shape is
    /// what shows. A row that can't move says why in place of its
    /// destination.
    fn preview_row(&self, mv: &Move, track: &Track) -> Div {
        let root = track.root.clone().unwrap_or_default();
        let rel = |path: &Path| {
            path.strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned()
        };
        let (line, color) = match &mv.blocked {
            Some(blocked) => (blocked.label(), palette::text_faint()),
            None if mv.unchanged => (rox_i18n::t!("tags-rename-unchanged"), palette::text_faint()),
            None => (SharedString::from(rel(&mv.to)), palette::text_bright()),
        };
        div()
            .flex()
            .flex_row()
            .items_start()
            .gap(tokens::SPACE_MD)
            .py(px(2.))
            .text_xs()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(palette::text_muted())
                    .child(SharedString::from(rel(&mv.from))),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(palette::text_faint())
                    .child(if mv.moves() { "→" } else { "·" }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(color)
                    .child(line),
            )
    }

    /// The pattern input, the placeholder help, and the remembered
    /// patterns as chips that fill the input when clicked.
    fn pattern_section(&self, cx: &mut Context<Self>) -> Div {
        let chips = self.remembered.iter().enumerate().map(|(i, pattern)| {
            let text = pattern.clone();
            div()
                .id(("remembered", i))
                .px(tokens::SPACE_XS)
                .py(px(1.))
                .rounded(tokens::RADIUS)
                .border_1()
                .border_color(palette::border())
                .text_xs()
                .text_color(palette::text_muted())
                .cursor_pointer()
                .hover(|d| d.text_color(palette::text()))
                .child(pattern.clone())
                .on_click(cx.listener(move |this, _, window, cx| {
                    let text = text.to_string();
                    this.pattern
                        .update(cx, |input, cx| input.set_value(text, window, cx));
                    this.replan(cx);
                }))
        });
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_XS)
            .child(Input::new(&self.pattern).small())
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!(
                        "tags-rename-pattern-help",
                        placeholders = guess::PLACEHOLDERS
                            .iter()
                            .filter(|p| **p != "%skip%")
                            .copied()
                            .collect::<Vec<_>>()
                            .join(" ")
                    )),
            )
            .when(!self.remembered.is_empty(), |d| {
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .flex_wrap()
                        .gap(tokens::SPACE_XS)
                        .children(chips),
                )
            })
    }

    /// The dialog's actions, and the shortcut for them. A run in flight,
    /// a pattern that won't parse, a move that failed, and a plan that
    /// moves nothing each take the shortcut's place, so the refusal is
    /// never silent.
    fn footer(&self, movable: usize, cx: &mut Context<Self>) -> Div {
        let reason = match (&self.parse_error, &self.error) {
            (Some(e), _) => Some((e.clone(), palette::tone_warn())),
            (None, Some(e)) => Some((e.clone(), palette::tone_bad())),
            (None, None) if movable == 0 => Some((
                rox_i18n::t!("tags-rename-nothing-to-move"),
                palette::tone_warn(),
            )),
            _ => None,
        };
        let hint = if self.applying {
            let at = (self.done + 1).min(self.total);
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_XS)
                .text_xs()
                .text_color(palette::text_muted())
                .child(Spinner::new().with_size(Size::Small))
                .child(rox_i18n::t!(
                    "tags-rename-moving",
                    done = at as u64,
                    total = self.total as u64
                ))
                .into_any_element()
        } else if let Some((reason, color)) = reason {
            div()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(color)
                .child(reason)
                .into_any_element()
        } else {
            kbd_line([
                Seg::Text("Press".into()),
                Seg::Key("Enter".into()),
                Seg::Text("to apply".into()),
            ])
            .text_xs()
            .into_any_element()
        };
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
            .child(hint)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(small_button(
                        "Apply",
                        icons::CHECK,
                        self.applying || movable == 0,
                        cx.listener(|this, _, window, cx| this.apply(window, cx)),
                    ))
                    // Cancel stays live through a run: every move is its
                    // own rename, so stopping leaves the files that moved
                    // where they moved to and the rest where they were.
                    .child(small_button(
                        "Cancel",
                        icons::CLOSE,
                        false,
                        cx.listener(|this, _, window, cx| {
                            this.persist_frame(window, cx);
                            window.remove_window();
                        }),
                    )),
            )
    }
}

impl Render for RenameFiles {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let movable = self.movable();
        let count = rox_i18n::t!(
            "tags-rename-will-move",
            count = movable as u64,
            total = self.tracks.len() as u64
        );
        let rows = self
            .plan
            .iter()
            .zip(&self.tracks)
            .map(|(mv, track)| self.preview_row(mv, track))
            .collect::<Vec<_>>();
        let preview = div()
            .flex_1()
            .min_h_0()
            .relative()
            .child(
                div()
                    .id("rename-preview")
                    .size_full()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(rows),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(Scrollbar::vertical(&self.scroll)),
            );

        div()
            .size_full()
            .flex()
            .flex_col()
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &Apply, window, cx| this.apply(window, cx)))
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            .children(self.backdrop.layer(&self.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .p(tokens::SPACE_MD)
                    // The body's own surface, a second elevated layer over
                    // the window's, the same as the settings page. The
                    // backdrop reads through two layers everywhere.
                    .bg(palette::bg_elevated())
                    .child(section(
                        rox_i18n::t!("tags-rename-pattern-section"),
                        None,
                        self.pattern_section(cx),
                    ))
                    .child(
                        section(
                            rox_i18n::t!("tags-rename-preview-section"),
                            Some(
                                div()
                                    .text_xs()
                                    .text_color(palette::text())
                                    .child(count)
                                    .into_any_element(),
                            ),
                            preview,
                        )
                        .flex_1()
                        .min_h_0(),
                    ),
            )
            .child(self.footer(movable, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(from: &str, sub: u16, root: Option<&str>, values: &[(Field, &str)]) -> Track {
        Track {
            from: PathBuf::from(from),
            sub,
            root: root.map(PathBuf::from),
            values: Some(
                values
                    .iter()
                    .map(|(f, v)| (f.clone(), (*v).to_owned()))
                    .collect(),
            ),
        }
    }

    fn album(from: &str, title: &str, no: &str) -> Track {
        track(
            from,
            0,
            Some("/m"),
            &[
                (Field::AlbumArtist, "Boards"),
                (Field::Album, "Geogaddi"),
                (Field::Title, title),
                (Field::TrackNo, no),
            ],
        )
    }

    fn run(tracks: &[Track], pattern: &str, taken: &[&str]) -> Vec<Move> {
        let taken: Vec<PathBuf> = taken.iter().map(PathBuf::from).collect();
        let pattern = guess::parse(pattern).unwrap();
        plan(tracks, &pattern, &|path| taken.iter().any(|t| t == path))
    }

    #[test]
    fn renders_under_the_track_own_root() {
        let got = run(
            &[album("/m/old/thing.flac", "Julie", "4")],
            "%albumartist%/%album%/%track% - %title%",
            &[],
        );
        assert_eq!(
            got[0].to,
            PathBuf::from("/m/Boards/Geogaddi/04 - Julie.flac")
        );
        assert!(got[0].moves());
    }

    #[test]
    fn a_file_already_at_its_destination_stays() {
        let got = run(
            &[album("/m/Boards/Geogaddi/04 - Julie.flac", "Julie", "4")],
            "%albumartist%/%album%/%track% - %title%",
            &[],
        );
        assert!(got[0].unchanged);
        assert!(!got[0].moves());
    }

    #[test]
    fn two_tracks_onto_one_name_both_refuse() {
        let got = run(
            &[
                album("/m/a.flac", "Julie", "4"),
                album("/m/b.flac", "Julie", "4"),
                album("/m/c.flac", "Candy", "5"),
            ],
            "%album%/%track% - %title%",
            &[],
        );
        assert_eq!(got[0].blocked, Some(Blocked::Duplicate));
        assert_eq!(got[1].blocked, Some(Blocked::Duplicate));
        assert!(got[2].moves());
    }

    #[test]
    fn an_occupied_destination_refuses() {
        let got = run(
            &[album("/m/a.flac", "Julie", "4")],
            "%album%/%track% - %title%",
            &["/m/Geogaddi/04 - Julie.flac"],
        );
        assert_eq!(got[0].blocked, Some(Blocked::Occupied));
    }

    #[test]
    fn a_cue_track_never_moves() {
        let mut cue = album("/m/disc.flac", "Julie", "4");
        cue.sub = 3;
        let got = run(&[cue], "%album%/%track% - %title%", &[]);
        assert_eq!(got[0].blocked, Some(Blocked::CueTrack));
    }

    #[test]
    fn a_track_outside_every_root_never_moves() {
        let mut stray = album("/elsewhere/a.flac", "Julie", "4");
        stray.root = None;
        let got = run(&[stray], "%album%/%track% - %title%", &[]);
        assert_eq!(got[0].blocked, Some(Blocked::OutsideRoots));
    }

    #[test]
    fn a_case_only_rename_takes_the_temp_hop() {
        // The source reads as taken, the same thing a case-insensitive
        // filesystem reports about the destination of a case-only rename.
        let got = run(
            &[album("/m/geogaddi/04 - julie.flac", "Julie", "4")],
            "%album%/%track% - %title%",
            &["/m/geogaddi/04 - julie.flac"],
        );
        assert_eq!(got[0].to, PathBuf::from("/m/Geogaddi/04 - Julie.flac"));
        assert!(got[0].case_only);
        assert!(got[0].moves());
    }

    #[test]
    fn a_track_the_catalog_cannot_resolve_never_moves() {
        // Every field would fall back and the file would end up at
        // "Unknown Artist/Unknown Album/00 - Untitled", which is worse
        // than where it started.
        let mut unknown = album("/m/a.flac", "Julie", "4");
        unknown.values = None;
        let got = run(&[unknown], "%albumartist%/%album%/%track% - %title%", &[]);
        assert_eq!(got[0].blocked, Some(Blocked::Unresolved));
    }

    #[test]
    fn a_pattern_that_cannot_render_says_so_per_track() {
        let got = run(&[album("/m/a.flac", "Julie", "4")], "%skip%/%title%", &[]);
        assert!(matches!(got[0].blocked, Some(Blocked::Render(_))));
    }

    /// The disk half on a real directory: the move digs out the folders
    /// the pattern named, the lyrics sidecar follows the audio into them,
    /// and a case-only rename gets there through the hop.
    #[test]
    fn a_move_digs_its_folders_and_takes_the_sidecar_along() {
        let dir = std::env::temp_dir().join("rox-rename-move");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let from = dir.join("old.flac");
        fs::write(&from, b"audio").unwrap();
        fs::write(dir.join("old.lrc"), b"[00:00.00] la").unwrap();

        let to = dir.join("Boards/Geogaddi/04 - Julie.flac");
        move_file(&from, &to, false).unwrap();
        assert_eq!(fs::read(&to).unwrap(), b"audio");
        assert!(!from.exists(), "the source is gone, not copied");

        let moved = move_sidecars(&from, &to);
        let lrc = dir.join("Boards/Geogaddi/04 - Julie.lrc");
        assert_eq!(moved, vec![(dir.join("old.lrc"), lrc.clone())]);
        assert!(lrc.exists() && !dir.join("old.lrc").exists());

        // Case-only, the one a case-insensitive filesystem would read as
        // renaming a file onto itself.
        let cased = dir.join("Boards/Geogaddi/04 - JULIE.flac");
        move_file(&to, &cased, true).unwrap();
        assert_eq!(fs::read(&cased).unwrap(), b"audio");
        let left: Vec<_> = fs::read_dir(cased.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .filter(|n| n.to_string_lossy().contains(CLONE_SUFFIX))
            .collect();
        assert!(left.is_empty(), "the hop file doesn't survive the move");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_extension_survives_a_dotted_name() {
        let got = run(
            &[track(
                "/m/a.flac",
                0,
                Some("/m"),
                &[(Field::Artist, "R.E.M."), (Field::Title, "Vol. 2")],
            )],
            "%artist% - %title%",
            &[],
        );
        assert_eq!(got[0].to, PathBuf::from("/m/R.E.M - Vol. 2.flac"));
    }
}
