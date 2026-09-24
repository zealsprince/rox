//! Renaming files from their tags, the guesser run backwards. A pattern
//! renders each track's tags into a path under the library root it's already
//! under, keeping its extension, and every move previews before any happen.
//! A pattern with no `/` renames in place. Apply moves the rows with
//! [`Library::rename_files`], so ids, ratings, play counts and playlist
//! membership survive.
//!
//! Values come off the projection, which doesn't carry %comment%, so that
//! falls back like any missing field.
//!
//! Refused rather than guessed: cue tracks (no file of their own), tracks
//! outside every root, tracks with no projection row, and any destination
//! that exists or that two tracks share. A shuffle within the selection
//! reads as occupied too, rather than ordering itself into a sequence that
//! half-finishes.
//!
//! Thumbnails and waveform peaks are keyed by path and regenerate after a
//! move. Lyrics sidecars travel with the file.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use gpui::{
    App, Bounds, Context, Div, Entity, Focusable as _, Global, KeyBinding, ScrollHandle,
    SharedString, Subscription, Window, WindowHandle, actions, div, prelude::*, px, size,
};
use gpui_component::input::{InputEvent, InputState};
use gpui_component::scroll::Scrollbar;
use gpui_component::spinner::Spinner;
use gpui_component::{Root, Sizable, Size};

use rox_core::settings::Settings;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_library::lyrics;
use rox_library::writer::{CLONE_SUFFIX, Field};
use rox_panel_api::panel::{self, AppState};
use rox_panel_kit::ui::{self as settings_ui, Seg, kbd_line, section, small_button};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::catalog::Library;

use crate::matching::{WindowRegistry, open_or_focus};
use crate::tags::guess;

const DEFAULT_PATTERN: &str = "%albumartist%/%album%/%track% - %title%";

const REMEMBERED: usize = 6;

actions!(rename, [Apply]);

const CONTEXT: &str = "RenameFiles";

/// Bound on the window root, so enter applies from anywhere. The input sees
/// the key first and propagates it; the guard in [`RenameFiles::apply`] eats
/// the second arrival.
pub fn bindings() -> Vec<KeyBinding> {
    vec![KeyBinding::new("enter", Apply, Some(CONTEXT))]
}

#[derive(Default)]
struct OpenRenamers(Vec<(Vec<i64>, WindowHandle<Root>)>);

impl Global for OpenRenamers {}

impl WindowRegistry for OpenRenamers {
    type Key = Vec<i64>;
    fn entries(&mut self) -> &mut Vec<(Vec<i64>, WindowHandle<Root>)> {
        &mut self.0
    }
}

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

struct Track {
    from: PathBuf,
    sub: u16,
    root: Option<PathBuf>,
    /// None without a projection row. Rendering it anyway would file the track
    /// under "Unknown Artist".
    values: Option<Vec<(Field, String)>>,
}

#[derive(Clone, Debug, PartialEq)]
enum Blocked {
    /// Moving a cue track would move the whole rip; the same rule as
    /// `writer::writes_to_file`.
    CueTrack,
    OutsideRoots,
    Unresolved,
    Render(String),
    Duplicate,
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

struct Move {
    from: PathBuf,
    to: PathBuf,
    /// Goes through a temp name, since a case-insensitive filesystem reads a
    /// case-only rename as a no-op.
    case_only: bool,
    unchanged: bool,
    blocked: Option<Blocked>,
}

impl Move {
    fn moves(&self) -> bool {
        self.blocked.is_none() && !self.unchanged
    }
}

/// Not `set_extension`, which would eat everything after a dot in a title
/// like "R.E.M.".
fn with_extension(path: PathBuf, ext: Option<&std::ffi::OsStr>) -> PathBuf {
    let Some(ext) = ext else { return path };
    let mut name: OsString = path.as_os_str().to_os_string();
    name.push(".");
    name.push(ext);
    PathBuf::from(name)
}

fn same_but_case(a: &Path, b: &Path) -> bool {
    a != b && a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

/// `exists` is injected so the plan tests without a filesystem.
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
        // A pattern with folders lays out from the root; a bare file name renames
        // in place.
        let base = if pattern.has_folders() {
            track.root.clone().unwrap_or_default()
        } else {
            track
                .from
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default()
        };
        let values = track.values.as_deref().unwrap_or_default();
        let to = match pattern.render(values) {
            Ok(rendered) => with_extension(base.join(rendered), track.from.extension()),
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
    // Two sources onto one destination: both refuse rather than one silently
    // winning.
    let mut seen: HashMap<PathBuf, usize> = HashMap::new();
    for mv in moves.iter().filter(|mv| mv.moves()) {
        *seen.entry(mv.to.clone()).or_default() += 1;
    }
    for mv in moves.iter_mut() {
        if mv.moves() && seen.get(&mv.to).copied().unwrap_or(0) > 1 {
            mv.blocked = Some(Blocked::Duplicate);
        }
    }
    // The file's own path, or its case-only variant, isn't a collision.
    for mv in moves.iter_mut() {
        if mv.moves() && !mv.case_only && exists(&mv.to) {
            mv.blocked = Some(Blocked::Occupied);
        }
    }
    moves
}

/// The writer's clone naming, so the watcher ignores the hop.
fn hop_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(CLONE_SUFFIX);
    path.with_file_name(name)
}

fn cross_device(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    let code = 18; // EXDEV
    #[cfg(windows)]
    let code = 17; // ERROR_NOT_SAME_DEVICE
    #[cfg(not(any(unix, windows)))]
    let code = -1;
    e.raw_os_error() == Some(code)
}

/// Across filesystems: copy to a clone beside the destination, flush, rename
/// into place, and only then unlink the original, so an interrupted copy
/// never costs the file.
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

/// Best effort: a sidecar that fails to move leaves the audio where it now
/// is. Returns the pairs that moved.
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
    pattern: Entity<InputState>,
    remembered: Vec<SharedString>,
    /// Rebuilt when the pattern changes, never per frame: it stats the disk.
    plan: Vec<Move>,
    parse_error: Option<SharedString>,
    error: Option<SharedString>,
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
                        .filter(|(row, _)| !projection.is_dead(*row as u32))
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
                // Roots never nest, so at most one matches.
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
                    // A zero means no number, so it renders as missing, not "00" or year 0.
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

    fn movable(&self) -> usize {
        self.plan.iter().filter(|mv| mv.moves()).count()
    }

    /// Each file is noted as a self-rename right before it moves; noting the
    /// whole batch up front would let the suppression window expire on a long
    /// run.
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
                            // Sidecars only follow a file that landed.
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
                    library.update(cx, |library, cx| library.rename_files(landed, cx));
                }
                match first_error {
                    None => {
                        this.persist_frame(window, cx);
                        window.remove_window();
                    }
                    Some(e) => {
                        // Replan against where the finished moves left things, so a retry doesn't
                        // re-move them.
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

    /// After a partial apply, point each landed track at its new path.
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

    /// Both paths relative to the root, so the pattern's shape is what shows.
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
        panel::pattern_input(
            "rename-pattern",
            &self.pattern,
            guess::PLACEHOLDERS,
            vec![rox_i18n::t!("tags-rename-pattern-help")],
            None,
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
                    // Cancel stays live: each move is its own rename, so stopping is safe.
                    .child(small_button(
                        rox_i18n::t!("settings-common-cancel"),
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
    fn a_pattern_without_folders_renames_in_place() {
        let got = run(
            &[album("/m/Boards/Geogaddi/Julie.flac", "Julie", "4")],
            "%track% - %title%",
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
        // The source reads as taken, as a case-insensitive filesystem would report.
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
