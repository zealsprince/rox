//! The cover art editor: the picture slots (front, back, media, artist)
//! across every selected file. A slot shows the shared image or a "multiple"
//! note. A save diffs each slot per file against that file's own pictures and
//! commits only what changed, through the tag editor's atomic layer.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    App, Bounds, Context, Div, Entity, FocusHandle, Global, Image, ImageFormat, KeyBinding,
    MouseButton, ObjectFit, PathPromptOptions, SharedString, Stateful, Subscription, Window,
    WindowHandle, actions, div, img, prelude::*, px, size,
};
use gpui_component::Root;

use rox_core::fmt::fmt_ms;
use rox_library::cue::{TrackKey, local};
use rox_library::writer::{self, Edit, PicChange, PicKind};

use crate::matching::{WindowRegistry, open_or_focus};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_net::providers;
use rox_panel_api::panel::AppState;
use rox_panel_kit::ui::{self as settings_ui, SECTION_GAP, Seg, kbd_line, section};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::catalog::Library;

const SLOTS: &[PicKind] = &[
    PicKind::Front,
    PicKind::Back,
    PicKind::Media,
    PicKind::Artist,
];

fn slot_label(kind: PicKind) -> SharedString {
    match kind {
        PicKind::Front => rox_i18n::t!("cover-editor-slot-front"),
        PicKind::Back => rox_i18n::t!("cover-editor-slot-back"),
        PicKind::Media => rox_i18n::t!("cover-editor-slot-media"),
        PicKind::Artist => "Artist".into(),
    }
}

const DEFAULT_SIZE: (f32, f32) = (560., 680.);

/// One name for every card: group bounds resolve innermost-first.
const SLOT_GROUP: &str = "cover-slot";

actions!(cover_editor, [Save]);

const CONTEXT: &str = "CoverEditor";

/// Nothing here takes typing, so the root holds focus and Enter saves.
pub fn bindings() -> Vec<KeyBinding> {
    vec![KeyBinding::new("enter", Save, Some(CONTEXT))]
}

#[derive(Default)]
struct OpenCoverEditors(Vec<(Vec<i64>, WindowHandle<Root>)>);

impl Global for OpenCoverEditors {}

impl WindowRegistry for OpenCoverEditors {
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

type FilePictures = Vec<(PicKind, Vec<u8>, String)>;

/// `sub` says which row of a cue image the tags belong to.
struct CoverTrack {
    path: PathBuf,
    sub: u16,
    line: SharedString,
    duration_ms: u32,
}

enum Current {
    None,
    /// Only some files have one, or they hold different bytes.
    Mixed,
    Image(Arc<Image>),
}

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
    /// Parallel to `tracks`. None until every read lands; save stays inert without it.
    baselines: Option<Vec<FilePictures>>,
    /// One per [`SLOTS`].
    slots: Vec<Slot>,
    error: Option<SharedString>,
    saving: bool,
    save_done: usize,
    save_total: usize,
    /// No field takes typing, so this gives the Enter binding a dispatch path.
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

    /// One unreadable file blocks the save: there's nothing safe to diff against.
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

    /// A file that won't decode shows an error rather than arming the slot.
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

    /// The matcher calls back into [`Self::set_front`] rather than writing, so
    /// this editor stays the one writer.
    fn search_online(&mut self, cx: &mut Context<Self>) {
        let Some(track) = self.tracks.first() else {
            return;
        };
        let key = TrackKey {
            source: local(),
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

    /// Arms the front slot as the user's pick; the normal save embeds it.
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

    fn removable(&self, slot: usize) -> bool {
        matches!(self.slots[slot].action, Action::Set { .. })
            || (matches!(self.slots[slot].action, Action::Keep)
                && !matches!(self.slots[slot].current, Current::None))
    }

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
            // One file per background hop: the count moves, a slow file shows, and a
            // closed window ends the loop.
            let mut committed: Vec<Edit> = Vec::new();
            let mut failures = 0usize;
            let mut first_error: Option<String> = None;
            for edit in edits {
                // Note the write first so its watch batch is suppressed. apply_edits notes
                // too, but too late for all but the last files of a big batch.
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
                // Baselines follow the writes, so a retry doesn't re-commit what succeeded.
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
                    // No subs: a cover edit names no columns; the reindex picks the picture up.
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

    fn cover_section(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let search = providers::art_online().then(|| {
            settings_ui::small_button(
                rox_i18n::t!("cover-editor-search-online"),
                icons::DOWNLOAD,
                self.saving || self.baselines.is_none(),
                cx.listener(|this, _, _, cx| this.search_online(cx)),
            )
            .into_any_element()
        });
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
                    row = row.child(div().flex_1());
                }
                row
            }),
        );
        section(
            rox_i18n::t!("cover-editor-section"),
            search,
            // Lock the cards during a commit. Cancel stays reachable in the footer.
            div().relative().child(cards).when(self.saving, |d| {
                d.child(div().absolute().inset_0().occlude())
            }),
        )
    }

    fn footer(&self, cx: &mut Context<Self>) -> Div {
        let reason: Option<SharedString> = if let Some(error) = self.error.clone() {
            Some(error)
        } else if self.saving {
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
                    // Cancel stays live through a save: the atomic writer leaves every original
                    // intact whether the batch finished or not.
                    .child(settings_ui::small_button(
                        rox_i18n::t!("settings-common-cancel"),
                        icons::CLOSE,
                        false,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    )),
            )
    }

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

fn art(image: Arc<Image>) -> Div {
    div()
        .size_full()
        .child(img(image).size_full().object_fit(ObjectFit::Contain))
}

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

/// None when gpui can't decode the mime.
pub(crate) fn decode(bytes: &[u8], mime: &str) -> Option<Arc<Image>> {
    let format = ImageFormat::from_mime_type(mime)?;
    Some(Arc::new(Image::from_bytes(format, bytes.to_vec())))
}

/// The formats gpui can embed and decode, the same sniff the art module runs.
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
