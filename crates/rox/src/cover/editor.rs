//! The cover art editor window: one OS window opened on a selection, the
//! same shape as the tag editor but for pictures. It edits the curated
//! picture slots a music library keeps (front cover, back cover, media,
//! artist) and applies each change to every selected file, so retagging a
//! whole album's art is one pass. A slot shows the selection's current
//! image when every file agrees, a "multiple" note when they differ, and a
//! replace or remove acts on all of them. Baselines come off each file
//! through the writer's picture read, so a save diffs per file and commits
//! only the slots that actually changed, through the same atomic layer the
//! tag editor uses. A successful save applies in one batch and refreshes the
//! art caches through the library reload, no manual invalidation.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    actions, div, img, prelude::*, px, size, App, Bounds, Context, Div, Entity, FocusHandle,
    Global, Image, ImageFormat, KeyBinding, MouseButton, ObjectFit, PathPromptOptions,
    SharedString, Stateful, Subscription, Window, WindowHandle,
};
use gpui_component::Root;

use rox_core::fmt::fmt_ms;
use rox_library::cue::TrackKey;
use rox_library::writer::{self, Edit, PicChange, PicKind};

use crate::matching::{open_or_focus, WindowRegistry};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_net::providers;
use rox_panel_api::panel::AppState;
use rox_panel_kit::ui::{self as settings_ui, kbd_line, section, Seg, SECTION_GAP};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::catalog::Library;

/// The picture slots the editor exposes, in display order.
const SLOTS: &[PicKind] = &[
    PicKind::Front,
    PicKind::Back,
    PicKind::Media,
    PicKind::Artist,
];

/// The label a slot shows over its preview.
fn slot_label(kind: PicKind) -> SharedString {
    match kind {
        PicKind::Front => rox_i18n::t!("cover-editor-slot-front"),
        PicKind::Back => rox_i18n::t!("cover-editor-slot-back"),
        PicKind::Media => rox_i18n::t!("cover-editor-slot-media"),
        PicKind::Artist => "Artist".into(),
    }
}

/// The default window size; wide enough for the four slot cards to fit two
/// across without scrolling.
const DEFAULT_SIZE: (f32, f32) = (560., 680.);

/// The hover group each slot's preview shares, so an upload prompt fades in
/// over the card the pointer is on. One name for every card: group bounds
/// resolve innermost-first, so each card scopes the hover to itself.
const SLOT_GROUP: &str = "cover-slot";

actions!(cover_editor, [Save]);

/// The key context the window's own bindings scope to.
const CONTEXT: &str = "CoverEditor";

/// The editor's save binding; call once at startup, before
/// [`crate::keymap::init`] snapshots what's bound. Nothing here takes
/// typing, so the binding is on the window root and the root holds the
/// focus, which puts it on the dispatch path.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("enter", Save, Some(CONTEXT))]);
}

/// The open editors, each keyed by the sorted ids it opened on, so asking
/// for one already open focuses it instead of stacking a twin. The same
/// shape as the tag editor's registry.
#[derive(Default)]
struct OpenCoverEditors(Vec<(Vec<i64>, WindowHandle<Root>)>);

impl Global for OpenCoverEditors {}

impl WindowRegistry for OpenCoverEditors {
    type Key = Vec<i64>;
    fn entries(&mut self) -> &mut Vec<(Vec<i64>, WindowHandle<Root>)> {
        &mut self.0
    }
}

/// Open a cover editor on `ids`, or bring the one already on that
/// selection to the front. An empty selection opens nothing.
pub fn open(state: AppState, ids: Vec<i64>, cx: &mut App) {
    if ids.is_empty() {
        return;
    }
    let mut key = ids.clone();
    key.sort_unstable();
    open_or_focus::<OpenCoverEditors>(
        key,
        move |cx| {
            let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
            rox_panel_api::panel::open_child_window(
                cx,
                rox_i18n::t!("cover-editor-window-title"),
                bounds,
                Some(settings_ui::MIN_SIZE),
                move |window, cx| cx.new(|cx| CoverEditor::new(state, ids, window, cx)),
            )
        },
        cx,
    );
}

/// One file's embedded pictures at the editor's slots, as the writer reads
/// them: the parallel-to-tracks baseline a save diffs against.
type FilePictures = Vec<(PicKind, Vec<u8>, String)>;

/// One selected track as the list shows it; the baselines read the path and
/// the commits write it, and the sub says which row of it the tags
/// belong to when the file is a cue image.
struct CoverTrack {
    path: PathBuf,
    sub: u16,
    line: SharedString,
    duration_ms: u32,
}

/// The selection's current image at a slot, folded across the files.
enum Current {
    /// No file has a picture here.
    None,
    /// The files disagree: only some have one, or they hold different bytes.
    Mixed,
    /// Every file has the same image; its decoded texture.
    Image(Arc<Image>),
}

/// A pending edit to a slot, `Keep` until the user moves it.
enum Action {
    Keep,
    Remove,
    Set {
        bytes: Arc<Vec<u8>>,
        mime: String,
        image: Arc<Image>,
    },
}

struct Slot {
    current: Current,
    action: Action,
}

pub struct CoverEditor {
    library: Entity<Library>,
    tracks: Vec<CoverTrack>,
    /// Each file's pictures as the writer read them, parallel to `tracks`:
    /// what save diffs against, per file. None until every read comes in (or
    /// never, when a file defeats the parser), and save stays inert without
    /// it.
    baselines: Option<Vec<FilePictures>>,
    /// One entry per [`SLOTS`], seeded once the baselines arrive.
    slots: Vec<Slot>,
    /// A failed read or commit, shown in the footer in place of the
    /// shortcut.
    error: Option<SharedString>,
    /// A commit is in flight; the cards lock and the buttons hold still
    /// until it finishes.
    saving: bool,
    /// How many of the batch have committed and how many there are, for the
    /// "Saving n/m" count. A file at a time advances this, so a slow or
    /// stuck one shows where the batch is instead of a mute spinner.
    save_done: usize,
    save_total: usize,
    /// The window root's own focus. No field here takes typing, so without
    /// it the enter binding would have nothing to attach to.
    focus: FocusHandle,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    _backdrop_changed: Subscription,
}

impl CoverEditor {
    fn new(state: AppState, ids: Vec<i64>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tracks =
            {
                let library = state.library.read(cx);
                let projection = library.projection().cloned();
                let row_of = projection.as_ref().map(|projection| {
                    projection
                        .db_id
                        .iter()
                        .enumerate()
                        .filter(|(row, _)| !projection.is_dead(*row as u32))
                        .map(|(row, &id)| (id, row as u32))
                        .collect::<std::collections::HashMap<_, _>>()
                });
                let mut tracks = Vec::with_capacity(ids.len());
                for &id in &ids {
                    let Some(path) = library
                        .paths_for(&[id])
                        .ok()
                        .and_then(|mut paths| paths.pop())
                    else {
                        continue;
                    };
                    let resolved = projection.as_ref().zip(row_of.as_ref()).and_then(
                        |(projection, row_of)| {
                            let row = *row_of.get(&id)?;
                            let v = projection.resolve(row);
                            Some((
                                v.title.to_owned(),
                                v.artist.to_owned(),
                                v.duration_ms,
                                v.sub,
                            ))
                        },
                    );
                    let (title, artist, duration_ms, sub) = resolved.unwrap_or_else(|| {
                        let title = path
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string());
                        (title, String::new(), 0, 0)
                    });
                    let mut line = title;
                    if !artist.is_empty() {
                        line.push_str(" - ");
                        line.push_str(&artist);
                    }
                    tracks.push(CoverTrack {
                        path,
                        sub,
                        line: line.into(),
                        duration_ms,
                    });
                }
                tracks
            };
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        let focus = cx.focus_handle();
        window.focus(&focus);
        let this = CoverEditor {
            library: state.library,
            tracks,
            baselines: None,
            slots: SLOTS
                .iter()
                .map(|_| Slot {
                    current: Current::None,
                    action: Action::Keep,
                })
                .collect(),
            error: None,
            saving: false,
            save_done: 0,
            save_total: 0,
            focus,
            now_art: state.now_art,
            backdrop: WindowBackdrop::default(),
            _backdrop_changed,
        };
        this.read_baselines(window, cx);
        this
    }

    /// Read every file's pictures off the UI thread and fold them into the
    /// slots when they all come in. One unreadable file blocks the save:
    /// without its baseline there's nothing safe to diff against.
    fn read_baselines(&self, window: &mut Window, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self.tracks.iter().map(|track| track.path.clone()).collect();
        cx.spawn_in(window, async move |this, cx| {
            let reads = cx
                .background_executor()
                .spawn(async move {
                    paths
                        .iter()
                        .map(|path| writer::read_pictures(path))
                        .collect::<Vec<_>>()
                })
                .await;
            this.update_in(cx, |this, _, cx| {
                let mut baselines = Vec::with_capacity(reads.len());
                for (read, track) in reads.into_iter().zip(&this.tracks) {
                    match read {
                        Ok(pictures) => baselines.push(pictures),
                        Err(e) => {
                            let name = track
                                .path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| track.path.display().to_string());
                            this.error = Some(format!("{name}: {e}").into());
                            cx.notify();
                            return;
                        }
                    }
                }
                this.fill(baselines, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Fold the finished baselines into each slot's current image: every
    /// file holding the same bytes shows that image, a split shows the mixed
    /// note, all-empty shows nothing.
    fn fill(&mut self, baselines: Vec<FilePictures>, cx: &mut Context<Self>) {
        for (i, kind) in SLOTS.iter().enumerate() {
            let mut present = baselines.iter().map(|pictures| {
                pictures
                    .iter()
                    .find(|(k, _, _)| k == kind)
                    .map(|(_, data, mime)| (data, mime))
            });
            let first = present.next().flatten();
            let agree = present.all(|other| other.map(|(d, _)| d) == first.map(|(d, _)| d));
            self.slots[i].current = match (agree, first) {
                (false, _) => Current::Mixed,
                (true, None) => Current::None,
                (true, Some((data, mime))) => match decode(data, mime) {
                    Some(image) => Current::Image(image),
                    None => Current::Mixed,
                },
            };
        }
        self.baselines = Some(baselines);
        cx.notify();
    }

    /// Pick an image file for a slot and load it off the UI thread. A
    /// picked file that won't decode shows the error rather than arming
    /// a slot with something the write couldn't embed.
    fn pick(&mut self, slot: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(rox_i18n::t!("cover-editor-choose-image")),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = rx.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    let bytes = std::fs::read(&path).ok()?;
                    let mime = sniff_mime(&bytes)?.to_string();
                    Some((bytes, mime))
                })
                .await;
            this.update_in(cx, |this, _, cx| {
                match loaded {
                    Some((bytes, mime)) => {
                        let image = Arc::new(Image::from_bytes(
                            ImageFormat::from_mime_type(&mime).unwrap_or(ImageFormat::Png),
                            bytes.clone(),
                        ));
                        this.slots[slot].action = Action::Set {
                            bytes: Arc::new(bytes),
                            mime,
                            image,
                        };
                        this.error = None;
                    }
                    None => {
                        this.error = Some(rox_i18n::t!("cover-editor-not-an-image"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Open the cover search on the selection's album. The picker fetches
    /// candidates, and on apply calls back into [`Self::set_front`] rather
    /// than writing, so this editor stays the one writer, the tag editor's
    /// fill shape. The query is the first track's artist and album.
    fn search_online(&mut self, cx: &mut Context<Self>) {
        let Some(track) = self.tracks.first() else {
            return;
        };
        let key = TrackKey {
            path: track.path.clone(),
            sub: track.sub,
        };
        let (artist, album) = self
            .library
            .read(cx)
            .meta_for_key(&key)
            .map(|m| (m.artist, m.album))
            .unwrap_or_default();
        crate::cover::matcher::open(
            self.now_art.clone(),
            cx.entity().downgrade(),
            artist,
            album,
            cx,
        );
    }

    /// Set the front cover from a fetched image: decode it, arm the front
    /// slot as the user's pick, so the normal save embeds it. Called by
    /// the cover picker on its own apply. An image that won't decode
    /// leaves the slot alone and shows why.
    pub fn set_front(&mut self, bytes: Vec<u8>, mime: String, cx: &mut Context<Self>) {
        let Some(front) = SLOTS.iter().position(|kind| *kind == PicKind::Front) else {
            return;
        };
        match decode(&bytes, &mime) {
            Some(image) => {
                self.slots[front].action = Action::Set {
                    bytes: Arc::new(bytes),
                    mime,
                    image,
                };
                self.error = None;
            }
            None => self.error = Some(rox_i18n::t!("cover-editor-not-decoded")),
        }
        cx.notify();
    }

    /// Whether a slot holds anything to remove: an image the files have,
    /// or a replacement the user just picked.
    fn removable(&self, slot: usize) -> bool {
        matches!(self.slots[slot].action, Action::Set { .. })
            || (matches!(self.slots[slot].action, Action::Keep)
                && !matches!(self.slots[slot].current, Current::None))
    }

    /// Commit the armed slots: each slot the user moved diffs per file
    /// against that file's own pictures, so an unchanged slot never
    /// rewrites. The commits run through the writer's atomic layer off the
    /// UI thread; success applies the batch, refreshes the art caches through
    /// the library reload, and closes the window.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(baselines), false) = (&self.baselines, self.saving) else {
            return;
        };
        let mut edits = Vec::new();
        for (track, baseline) in self.tracks.iter().zip(baselines) {
            let mut pictures = Vec::new();
            for (i, kind) in SLOTS.iter().enumerate() {
                let current = baseline
                    .iter()
                    .find(|(k, _, _)| k == kind)
                    .map(|(_, d, _)| d);
                match &self.slots[i].action {
                    Action::Keep => {}
                    Action::Remove => {
                        if current.is_some() {
                            pictures.push(PicChange {
                                kind: *kind,
                                data: None,
                            });
                        }
                    }
                    Action::Set { bytes, mime, .. } => {
                        if current != Some(&**bytes) {
                            pictures.push(PicChange {
                                kind: *kind,
                                data: Some(((**bytes).clone(), mime.clone())),
                            });
                        }
                    }
                }
            }
            if !pictures.is_empty() {
                edits.push(Edit {
                    path: track.path.clone(),
                    changes: Vec::new(),
                    pictures,
                });
            }
        }
        if edits.is_empty() {
            window.remove_window();
            return;
        }
        self.saving = true;
        self.save_done = 0;
        self.save_total = edits.len();
        self.error = None;
        cx.notify();
        let library = self.library.clone();
        cx.spawn_in(window, async move |this, cx| {
            // One file per background hop, not the whole batch behind a
            // single await: the count moves as each finishes, a slow file
            // is visibly the one holding things up, and a cancel that closes
            // the window ends the loop instead of grinding on unseen.
            let mut committed: Vec<Edit> = Vec::new();
            let mut failures = 0usize;
            let mut first_error: Option<String> = None;
            for edit in edits {
                // Note the write before it happens so the watch batch it
                // triggers is suppressed, not reindexed. The apply_edits at
                // the end notes too, but by then the suppression window has
                // long passed for all but the last few files of a big batch.
                if library
                    .update(cx, |library, _| {
                        library.note_self_write([edit.path.clone()])
                    })
                    .is_err()
                {
                    return;
                }
                let (edit, result) = cx
                    .background_executor()
                    .spawn(async move {
                        let r = writer::commit_with(&edit.path, &edit.changes, &edit.pictures);
                        (edit, r)
                    })
                    .await;
                match result {
                    Ok(()) => committed.push(edit),
                    Err(e) => {
                        failures += 1;
                        if first_error.is_none() {
                            let name = edit
                                .path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| edit.path.display().to_string());
                            first_error = Some(format!("{name}: {e}"));
                        }
                    }
                }
                // A closed window (the user cancelled) drops the handle;
                // stop rather than keep writing into nothing.
                if this
                    .update(cx, |this, cx| {
                        this.save_done += 1;
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
            }
            this.update_in(cx, move |this, window, cx| {
                // A written file's baseline follows the write, so a retry
                // after a partial failure diffs against what's on disk
                // now instead of re-committing the files that succeeded.
                for edit in &committed {
                    let Some(ix) = this.tracks.iter().position(|t| t.path == edit.path) else {
                        continue;
                    };
                    let Some(baseline) = this.baselines.as_mut().and_then(|b| b.get_mut(ix)) else {
                        continue;
                    };
                    for picture in &edit.pictures {
                        match &picture.data {
                            Some((bytes, mime)) => {
                                match baseline.iter_mut().find(|(k, _, _)| *k == picture.kind) {
                                    Some(entry) => {
                                        entry.1 = bytes.clone();
                                        entry.2 = mime.clone();
                                    }
                                    None => {
                                        baseline.push((picture.kind, bytes.clone(), mime.clone()))
                                    }
                                }
                            }
                            None => baseline.retain(|(k, _, _)| *k != picture.kind),
                        }
                    }
                }
                if !committed.is_empty() {
                    // No subs: a cover edit names no columns, so there's no
                    // library row for it to apply to. The reindex behind it
                    // picks the new picture up.
                    library.update(cx, |library, cx| library.apply_edits(&committed, &[], cx));
                }
                match first_error {
                    None => window.remove_window(),
                    Some(e) => {
                        this.saving = false;
                        this.error = Some(if failures > 1 {
                            rox_i18n::t!(
                                "cover-editor-save-errors",
                                count = failures as u64,
                                error = e
                            )
                        } else {
                            e.into()
                        });
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// The selection as a list: the display line filling left, the duration
    /// right, one hairline row per track, the tag editor's track section.
    fn track_section(&self) -> Stateful<Div> {
        let mut body = div().flex().flex_col();
        for track in &self.tracks {
            body = body.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_MD)
                    .py(tokens::SPACE_XS)
                    .border_b_1()
                    .border_color(palette::border())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(track.line.clone()),
                    )
                    .when(track.duration_ms > 0, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .text_color(palette::text_muted())
                                .child(fmt_ms(track.duration_ms)),
                        )
                    }),
            );
        }
        section(rox_i18n::t!("head-piece-tracks"), None, body)
    }

    /// The cover art section: the slot cards under a header with the online
    /// search in it.
    fn cover_section(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        // The online search is placed in the header as a tool of the section
        // it fills, gated on a cover-art provider being on, and sets the
        // front cover on apply.
        let search = providers::art_online().then(|| {
            settings_ui::small_button(
                rox_i18n::t!("cover-editor-search-online"),
                icons::DOWNLOAD,
                self.saving || self.baselines.is_none(),
                cx.listener(|this, _, _, cx| this.search_online(cx)),
            )
            .into_any_element()
        });
        // Two cards a row, each growing to fill its half so the previews
        // scale with the window instead of staying at a fixed size.
        let cards = div().flex().flex_col().gap(tokens::SPACE_MD).children(
            (0..SLOTS.len()).step_by(2).map(|i| {
                let mut row = div()
                    .flex()
                    .flex_row()
                    .gap(tokens::SPACE_MD)
                    .child(self.slot_card(i, cx).flex_1().min_w_0());
                if i + 1 < SLOTS.len() {
                    row = row.child(self.slot_card(i + 1, cx).flex_1().min_w_0());
                } else {
                    // An odd tail keeps its half rather than stretching wide.
                    row = row.child(div().flex_1());
                }
                row
            }),
        );
        section(
            rox_i18n::t!("cover-editor-section"),
            search,
            // The cards lock while a commit is in flight: a transparent
            // occluder over them swallows clicks so no slot edits out from
            // under the write. Cancel is below it, in the footer.
            div().relative().child(cards).when(self.saving, |d| {
                d.child(div().absolute().inset_0().occlude())
            }),
        )
    }

    /// The window's own actions: the save, the shortcut for it, and what's
    /// holding it up when something is: a read still running, a commit in
    /// flight, or the write that failed.
    fn footer(&self, cx: &mut Context<Self>) -> Div {
        let reason: Option<SharedString> = if let Some(error) = self.error.clone() {
            Some(error)
        } else if self.saving {
            // A commit runs off the UI thread and a file at a time, so say
            // where the batch is rather than showing nothing.
            Some({
                let at = (self.save_done + 1).min(self.save_total);
                rox_i18n::t!(
                    "cover-editor-saving-progress",
                    done = at as u64,
                    total = self.save_total as u64
                )
            })
        } else if self.baselines.is_none() {
            Some(rox_i18n::t!("cover-editor-reading"))
        } else {
            None
        };
        let hint = match reason {
            Some(reason) => div()
                .text_xs()
                .text_color(palette::tone_warn())
                .child(reason)
                .into_any_element(),
            None => kbd_line([
                Seg::Text("Press".into()),
                Seg::Key("Enter".into()),
                Seg::Text("to save".into()),
            ])
            .text_xs()
            .into_any_element(),
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
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(settings_ui::small_button(
                        "Save",
                        icons::CHECK,
                        self.saving || self.baselines.is_none(),
                        cx.listener(|this, _, window, cx| this.save(window, cx)),
                    ))
                    // Cancel stays live through a save: a slow or wedged
                    // commit needs a way out, and the atomic writer leaves
                    // every original intact whether the batch finished or
                    // not.
                    .child(settings_ui::small_button(
                        rox_i18n::t!("settings-common-cancel"),
                        icons::CLOSE,
                        false,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    )),
            )
    }

    /// One slot: a preview of the effective image (the pick, the pending
    /// removal, or the file's current cover) that picks a replacement on
    /// click, with an upload prompt fading in on hover, and remove and
    /// revert actions under the slot label.
    fn slot_card(&self, slot: usize, cx: &mut Context<Self>) -> Div {
        let label = slot_label(SLOTS[slot]);
        let content: gpui::AnyElement = match &self.slots[slot].action {
            Action::Set { image, .. } => art(image.clone()).into_any_element(),
            Action::Remove => placeholder(icons::TRASH, rox_i18n::t!("cover-editor-will-remove"))
                .into_any_element(),
            Action::Keep => match &self.slots[slot].current {
                Current::Image(image) => art(image.clone()).into_any_element(),
                Current::Mixed => placeholder(icons::IMAGE, rox_i18n::t!("cover-editor-multiple"))
                    .into_any_element(),
                Current::None => {
                    placeholder(icons::IMAGE, rox_i18n::t!("cover-editor-none")).into_any_element()
                }
            },
        };
        let mut preview = div()
            .group(SLOT_GROUP)
            .id(("cover-slot", slot))
            .relative()
            .w_full()
            .rounded(tokens::RADIUS)
            .border_1()
            .border_color(palette::border())
            .bg(palette::bg_root())
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(content),
            );
        preview.style().aspect_ratio = Some(1.0);
        let preview = preview.when(!self.saving, |d| {
            d.cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| this.pick(slot, window, cx)),
                )
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap(tokens::SPACE_XS)
                        .bg(palette::alpha(palette::bg_root(), 0xCC))
                        .text_color(palette::text_bright())
                        .opacity(0.)
                        .group_hover(SLOT_GROUP, |s| s.opacity(1.))
                        .child(gpui::svg().path(icons::UPLOAD).size(px(24.)))
                        .child(div().text_xs().child(rox_i18n::t!("cover-editor-replace"))),
                )
        });
        let actions = div()
            .flex()
            .flex_row()
            .gap(tokens::SPACE_XS)
            .when(self.removable(slot), |d| {
                d.child(settings_ui::small_button(
                    rox_i18n::t!("cover-editor-remove"),
                    icons::TRASH,
                    self.saving,
                    cx.listener(move |this, _, _, cx| {
                        this.slots[slot].action = Action::Remove;
                        cx.notify();
                    }),
                ))
            })
            .when(!matches!(self.slots[slot].action, Action::Keep), |d| {
                d.child(settings_ui::small_button(
                    rox_i18n::t!("cover-editor-revert"),
                    icons::CLOSE,
                    self.saving,
                    cx.listener(move |this, _, _, cx| {
                        this.slots[slot].action = Action::Keep;
                        cx.notify();
                    }),
                ))
            });
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(preview)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(div().text_color(palette::text_muted()).child(label))
                    .child(actions),
            )
    }
}

/// A decoded image letterboxed into the preview square: `object_fit`
/// contains it within the box, preserving the image's own aspect.
fn art(image: Arc<Image>) -> Div {
    div()
        .size_full()
        .child(img(image).size_full().object_fit(ObjectFit::Contain))
}

/// The empty preview stand-in: a faint glyph over a one-word note.
fn placeholder(icon: &'static str, note: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(tokens::SPACE_XS)
        .text_color(palette::text_faint())
        .child(gpui::svg().path(icon).size(px(28.)))
        .child(div().text_xs().child(note.into()))
}

/// The image texture for a preview, decoded from the encoded bytes; None
/// when the mime names a format gpui can't decode.
pub(crate) fn decode(bytes: &[u8], mime: &str) -> Option<Arc<Image>> {
    let format = ImageFormat::from_mime_type(mime)?;
    Some(Arc::new(Image::from_bytes(format, bytes.to_vec())))
}

/// The mime type off an image's magic bytes, the set gpui can embed and
/// decode. The same sniff the art module runs on read.
pub(crate) fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif")
    } else if bytes.starts_with(b"BM") {
        Some("image/bmp")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

impl Render for CoverEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Save, window, cx| this.save(window, cx)))
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            // The backdrop paints first, under the page, so translucent
            // surfaces back with the playing track's art like every window.
            .children(self.backdrop.layer(&self.now_art, window, cx))
            .child(
                div()
                    .id("cover-editor-page")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .bg(palette::bg_elevated())
                    .p(tokens::SPACE_MD)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(SECTION_GAP)
                            .child(self.cover_section(cx))
                            .child(self.track_section()),
                    ),
            )
            .child(self.footer(cx))
    }
}
