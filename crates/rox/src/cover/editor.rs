//! The cover art editor window: one OS window opened on a selection, the
//! same shape as the tag editor but for pictures. It edits the curated
//! picture slots a music library carries - front cover, back cover, media,
//! artist - and applies each change to every selected file, so retagging a
//! whole album's art is one pass. A slot shows the selection's current
//! image when every file agrees, a "multiple" note when they differ, and a
//! replace or remove acts on all of them. Baselines come off each file
//! through the writer's picture read, so a save diffs per file and commits
//! only the slots that actually changed, through the same atomic layer the
//! tag editor uses. A successful save lands in one batch and refreshes the
//! art caches through the library reload, no manual invalidation.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    div, img, prelude::*, px, size, App, Bounds, Context, Div, Entity, Global, Image, ImageFormat,
    MouseButton, ObjectFit, PathPromptOptions, SharedString, Subscription, Window, WindowHandle,
};
use gpui_component::spinner::Spinner;
use gpui_component::{Root, Sizable, Size};

use rox_library::writer::{self, Edit, PicChange, PicKind};

use crate::assets::icons;
use crate::backdrop::{NowPlayingArt, WindowBackdrop};
use crate::design::{palette, tokens};
use crate::matching::{open_or_focus, WindowRegistry};
use crate::panel::AppState;
use crate::panels::library::{fmt_ms, Library};
use crate::providers;
use crate::settings::ui::{self as settings_ui, section, SECTION_GAP};

/// The picture slots the editor exposes, in display order: the label each
/// wears over its preview.
const SLOTS: &[(PicKind, &str)] = &[
    (PicKind::Front, "Front Cover"),
    (PicKind::Back, "Back Cover"),
    (PicKind::Media, "Media"),
    (PicKind::Artist, "Artist"),
];

/// The default window size; wide enough for the four slot cards to sit two
/// across without scrolling.
const DEFAULT_SIZE: (f32, f32) = (560., 680.);

/// The hover group each slot's preview shares, so an upload prompt fades in
/// over the card the pointer is on. One name for every card: group bounds
/// resolve innermost-first, so each card scopes the hover to itself.
const SLOT_GROUP: &str = "cover-slot";

/// The open editors, each keyed by the sorted ids it opened on, so asking
/// for one already open focuses it instead of stacking a twin - mirrors
/// the tag editor's registry.
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
            crate::panel::open_child_window(cx, "rox - Cover Art", bounds, Some(settings_ui::MIN_SIZE), move |window, cx| {
                cx.new(|cx| CoverEditor::new(state, ids, window, cx))
            })
        },
        cx,
    );
}

/// One file's embedded pictures at the editor's slots, as the writer reads
/// them: the parallel-to-tracks baseline a save diffs against.
type FilePictures = Vec<(PicKind, Vec<u8>, String)>;

/// One selected track as the list shows it; the path is what the baselines
/// read and the commits write.
struct CoverTrack {
    path: PathBuf,
    line: SharedString,
    duration_ms: u32,
}

/// The selection's current image at a slot, folded across the files.
enum Current {
    /// No file carries a picture here.
    None,
    /// The files disagree - some carry one, or they carry different bytes.
    Mixed,
    /// Every file carries the same image; its decoded texture.
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
    /// what save diffs against, per file. None until every read lands (or
    /// never, when a file defeats the parser), and save stays inert without
    /// it.
    baselines: Option<Vec<FilePictures>>,
    /// One entry per [`SLOTS`], seeded once the baselines land.
    slots: Vec<Slot>,
    /// A failed read or commit, shown inline over the buttons.
    error: Option<SharedString>,
    /// A commit is in flight; the cards lock and the buttons hold still
    /// until it lands.
    saving: bool,
    /// How many of the batch have committed and how many there are, for the
    /// "Saving n/m" count. A file at a time advances this, so a slow or
    /// stuck one shows where the batch is instead of a mute spinner.
    save_done: usize,
    save_total: usize,
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
                            Some((v.title.to_owned(), v.artist.to_owned(), v.duration_ms))
                        },
                    );
                    let (title, artist, duration_ms) = resolved.unwrap_or_else(|| {
                        let title = path
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string());
                        (title, String::new(), 0)
                    });
                    let mut line = title;
                    if !artist.is_empty() {
                        line.push_str(" - ");
                        line.push_str(&artist);
                    }
                    tracks.push(CoverTrack {
                        path,
                        line: line.into(),
                        duration_ms,
                    });
                }
                tracks
            };
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
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
            now_art: state.now_art,
            backdrop: WindowBackdrop::default(),
            _backdrop_changed,
        };
        this.read_baselines(window, cx);
        this
    }

    /// Read every file's pictures off the UI thread and fold them into the
    /// slots when they all land. One unreadable file blocks the save:
    /// without its baseline there is nothing safe to diff against.
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

    /// Fold the landed baselines into each slot's current image: every file
    /// carrying the same bytes shows that image, a split shows the mixed
    /// note, all-empty shows nothing.
    fn fill(&mut self, baselines: Vec<FilePictures>, cx: &mut Context<Self>) {
        for (i, (kind, _)) in SLOTS.iter().enumerate() {
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
    /// picked file that will not decode shows the error rather than arming
    /// a slot with something the write could not carry.
    fn pick(&mut self, slot: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose Image".into()),
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
                    None => this.error = Some("That file is not an image rox can embed".into()),
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
        let (artist, album) = self
            .library
            .read(cx)
            .meta_for(&track.path)
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
    /// the cover picker on its own apply. An image that will not decode
    /// leaves the slot alone and shows why.
    pub fn set_front(&mut self, bytes: Vec<u8>, mime: String, cx: &mut Context<Self>) {
        let Some(front) = SLOTS.iter().position(|(kind, _)| *kind == PicKind::Front) else {
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
            None => self.error = Some("That image could not be decoded".into()),
        }
        cx.notify();
    }

    /// Whether a slot holds anything to remove: an image the files carry,
    /// or a replacement the user just picked.
    fn removable(&self, slot: usize) -> bool {
        matches!(self.slots[slot].action, Action::Set { .. })
            || (matches!(self.slots[slot].action, Action::Keep)
                && !matches!(self.slots[slot].current, Current::None))
    }

    /// Commit the armed slots: each slot the user moved diffs per file
    /// against that file's own pictures, so an unchanged slot never
    /// rewrites. The commits run through the writer's atomic layer off the
    /// UI thread; success lands the batch, refreshes the art caches through
    /// the library reload, and closes the window.
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(baselines), false) = (&self.baselines, self.saving) else {
            return;
        };
        let mut edits = Vec::new();
        for (track, baseline) in self.tracks.iter().zip(baselines) {
            let mut pictures = Vec::new();
            for (i, (kind, _)) in SLOTS.iter().enumerate() {
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
            // single await: the count moves as each lands, a slow file is
            // visibly the one holding things up, and a cancel that closes
            // the window ends the loop instead of grinding on unseen.
            let mut committed: Vec<Edit> = Vec::new();
            let mut failures = 0usize;
            let mut first_error: Option<String> = None;
            for edit in edits {
                // Note the write before it lands so the watch batch it
                // triggers is suppressed, not reindexed. The apply_edits at
                // the end notes too, but by then the suppression window has
                // long passed for all but the last few files of a big batch.
                if library
                    .update(cx, |library, _| library.note_self_write([edit.path.clone()]))
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
                // after a partial failure diffs against what is on disk
                // now instead of re-committing the landed files.
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
                    library.update(cx, |library, cx| library.apply_edits(&committed, cx));
                }
                match first_error {
                    None => window.remove_window(),
                    Some(e) => {
                        this.saving = false;
                        this.error = Some(if failures > 1 {
                            format!("{failures} files failed; {e}").into()
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
    /// right, one hairline row per track - the tag editor's track section.
    fn track_section(&self) -> Div {
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
        section("Tracks", None, body)
    }

    /// The cover art section: the slot cards, save and cancel on the header,
    /// the error inline under the cards.
    fn cover_section(&self, cx: &mut Context<Self>) -> Div {
        let buttons = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            // A commit runs off the UI thread, so say it plainly: the
            // spinner and a running count ride ahead of the buttons until
            // the write lands or fails.
            .when(self.saving, |d| {
                let label = if self.save_total > 1 {
                    let at = (self.save_done + 1).min(self.save_total);
                    format!("Saving {}/{}...", at, self.save_total)
                } else {
                    "Saving...".to_string()
                };
                d.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_XS)
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(Spinner::new().with_size(Size::Small))
                        .child(label),
                )
            })
            // The online search rides the header, gated on a cover-art
            // provider being on, and sets the front cover on apply.
            .when(providers::art_online(), |d| {
                d.child(settings_ui::small_button(
                    "Search Online",
                    icons::DOWNLOAD,
                    self.saving || self.baselines.is_none(),
                    cx.listener(|this, _, _, cx| this.search_online(cx)),
                ))
            })
            .child(settings_ui::small_button(
                "Save",
                icons::CHECK,
                self.saving || self.baselines.is_none(),
                cx.listener(|this, _, window, cx| this.save(window, cx)),
            ))
            // Cancel stays live through a save: a slow or wedged commit
            // needs a way out, and the atomic writer leaves every original
            // intact whether the batch finished or not.
            .child(settings_ui::small_button(
                "Cancel",
                icons::CLOSE,
                false,
                cx.listener(|_, _, window, _| window.remove_window()),
            ))
            .into_any_element();
        // Two cards a row, each growing to fill its half so the previews
        // scale with the window instead of sitting at a fixed size.
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
            "Cover Art",
            Some(buttons),
            div()
                .flex()
                .flex_col()
                .child(
                    // The cards lock while a commit is in flight: a
                    // transparent occluder over them swallows clicks so no
                    // slot edits out from under the write. Cancel sits above
                    // it, on the header.
                    div()
                        .relative()
                        .child(cards)
                        .when(self.saving, |d| {
                            d.child(div().absolute().inset_0().occlude())
                        }),
                )
                .when_some(self.error.clone(), |d, error| {
                    d.child(
                        div()
                            .mt(tokens::SPACE_SM)
                            .text_color(palette::text_muted())
                            .child(error),
                    )
                }),
        )
    }

    /// One slot: a preview of the effective image (the pick, the pending
    /// removal, or the file's current cover) that picks a replacement on
    /// click, with an upload prompt fading in on hover, and remove and
    /// revert actions under the slot label.
    fn slot_card(&self, slot: usize, cx: &mut Context<Self>) -> Div {
        let (_, label) = SLOTS[slot];
        let content: gpui::AnyElement = match &self.slots[slot].action {
            Action::Set { image, .. } => art(image.clone()).into_any_element(),
            Action::Remove => placeholder(icons::TRASH, "Will remove").into_any_element(),
            Action::Keep => match &self.slots[slot].current {
                Current::Image(image) => art(image.clone()).into_any_element(),
                Current::Mixed => placeholder(icons::IMAGE, "Multiple").into_any_element(),
                Current::None => placeholder(icons::IMAGE, "None").into_any_element(),
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
                        .child(div().text_xs().child("Replace")),
                )
        });
        let actions = div()
            .flex()
            .flex_row()
            .gap(tokens::SPACE_XS)
            .when(self.removable(slot), |d| {
                d.child(settings_ui::small_button(
                    "Remove",
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
                    "Revert",
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
fn placeholder(icon: &'static str, note: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(tokens::SPACE_XS)
        .text_color(palette::text_faint())
        .child(gpui::svg().path(icon).size(px(28.)))
        .child(div().text_xs().child(note))
}

/// The image texture for a preview, decoded from the encoded bytes; None
/// when the mime names a format gpui cannot decode.
pub(crate) fn decode(bytes: &[u8], mime: &str) -> Option<Arc<Image>> {
    let format = ImageFormat::from_mime_type(mime)?;
    Some(Arc::new(Image::from_bytes(format, bytes.to_vec())))
}

/// The mime type off an image's magic bytes, the set gpui can embed and
/// decode - the same sniff the art module runs on read.
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
            .flex_row()
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
                    .min_w_0()
                    .h_full()
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
    }
}
