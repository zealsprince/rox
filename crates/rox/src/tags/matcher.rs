//! The metadata match window: an online lookup verified field by field
//! before it writes. Candidates list best first; the selected one shows as a
//! compare table where each field arms separately. Apply writes only the
//! armed fields through the tag editor's atomic commit. One window per track
//! and opening editor.

use gpui::{
    AnyWindowHandle, App, Bounds, Context, Div, Entity, EntityId, Global, ScrollHandle,
    SharedString, Subscription, Task, WeakEntity, Window, WindowHandle, div, prelude::*, px, size,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Root, Sizable as _};

use rox_library::cue::TrackKey;
use rox_library::writer::{self, Change, Edit, Field};

use crate::matching::{
    Phase, WindowRegistry, confidence_badge, confidence_bar, note, open_or_focus,
};
use crate::tags::editor::TagEditor;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_net::providers::{self, MetadataCandidate, TrackQuery};
use rox_panel_kit::ui::{self as settings_ui, SECTION_GAP, section};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::catalog::Library;
use rox_services::player::fmt_time;

/// The fill keeps an open tag editor the single writer, so the compare never
/// writes behind its back and leaves its baselines stale.
enum Sink {
    Commit,
    /// `track` names the editor row this ran on, so a fill lands in that row
    /// rather than over the batch.
    Fill {
        editor: WeakEntity<TagEditor>,
        window: AnyWindowHandle,
        track: usize,
    },
}

/// Rating and lyrics stay out: a release lookup doesn't return them. Nor do
/// title and album sort, since MusicBrainz only has sort names for artists.
type Pull = fn(&MetadataCandidate) -> String;
const FIELDS: &[(Field, &str, Pull)] = &[
    (Field::Title, "Title", |c| c.title.clone()),
    (Field::Artist, "Artist", |c| c.artist.clone()),
    (Field::ArtistSort, "Artist Sort", |c| c.artist_sort.clone()),
    (Field::AlbumArtist, "Album Artist", |c| {
        c.album_artist.clone()
    }),
    (Field::AlbumArtistSort, "Album Artist Sort", |c| {
        c.album_artist_sort.clone()
    }),
    (Field::Album, "Album", |c| c.album.clone()),
    (Field::Year, "Year", |c| c.year.clone()),
    (Field::TrackNo, "Track", |c| c.track_no.clone()),
    (Field::DiscNo, "Disc", |c| c.disc_no.clone()),
];

const DEFAULT_SIZE: (f32, f32) = (760., 560.);

const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(350);

/// A fill binds its Apply to the editor that opened it, so two editors on
/// one track need their own windows. Keyed on the whole track, so cue
/// subsongs get a window each.
type MatchKey = (TrackKey, Option<EntityId>);

#[derive(Default)]
struct OpenMatchers(Vec<(MatchKey, WindowHandle<Root>)>);

impl Global for OpenMatchers {}

impl WindowRegistry for OpenMatchers {
    type Key = MatchKey;
    fn entries(&mut self) -> &mut Vec<(MatchKey, WindowHandle<Root>)> {
        &mut self.0
    }
}

pub fn open(library: Entity<Library>, now_art: Entity<NowPlayingArt>, key: TrackKey, cx: &mut App) {
    open_with(library, now_art, key, Sink::Commit, cx);
}

pub fn open_fill(
    library: Entity<Library>,
    now_art: Entity<NowPlayingArt>,
    key: TrackKey,
    track: usize,
    editor: WeakEntity<TagEditor>,
    editor_window: AnyWindowHandle,
    cx: &mut App,
) {
    let sink = Sink::Fill {
        editor,
        window: editor_window,
        track,
    };
    open_with(library, now_art, key, sink, cx);
}

fn open_with(
    library: Entity<Library>,
    now_art: Entity<NowPlayingArt>,
    key: TrackKey,
    sink: Sink,
    cx: &mut App,
) {
    let opener = match &sink {
        Sink::Commit => None,
        Sink::Fill { editor, .. } => Some(editor.entity_id()),
    };
    open_or_focus::<OpenMatchers>(
        (key.clone(), opener),
        move |cx| {
            let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
            rox_panel_api::panel::open_child_window(
                cx,
                rox_i18n::t!("tags-matcher-window-title"),
                bounds,
                Some(settings_ui::MIN_SIZE),
                move |window, cx| {
                    cx.new(|cx| TagMatch::new(library, now_art, key, sink, window, cx))
                },
            )
        },
        cx,
    );
}

struct TagMatch {
    library: Entity<Library>,
    sink: Sink,
    key: TrackKey,
    line: SharedString,
    /// Seeded from the tags; both the search and the score read them.
    artist_input: Entity<InputState>,
    title_input: Entity<InputState>,
    album: String,
    duration_secs: Option<f64>,
    /// Read once at open: the availability check may load the settings file,
    /// which has no place in a paint.
    can_identify: bool,
    /// Replacing it cancels the pending timer and any in-flight request.
    search_task: Option<Task<()>>,
    current: Vec<String>,
    phase: Phase<MetadataCandidate>,
    selected: Option<usize>,
    /// Reset on selection: on where the fetched value is non-empty and differs.
    armed: Vec<bool>,
    saving: bool,
    error: Option<SharedString>,
    scroll: ScrollHandle,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    _input_events: Vec<Subscription>,
    _backdrop_changed: Subscription,
}

impl TagMatch {
    fn new(
        library: Entity<Library>,
        now_art: Entity<NowPlayingArt>,
        key: TrackKey,
        sink: Sink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (meta, duration_secs) = {
            let lib = library.read(cx);
            let resolved = lib.resolve_key(&key);
            let duration_secs = resolved
                .as_ref()
                .and_then(|(id, _)| duration_secs_for(&library, *id, cx));
            (resolved.map(|(_, meta)| meta), duration_secs)
        };
        let (artist, title, album) = meta
            .map(|m| (m.artist, m.title, m.album))
            .unwrap_or_default();
        let line = if artist.is_empty() {
            title.clone()
        } else {
            format!("{title} - {artist}")
        };
        let artist_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("head-piece-artist"))
                .default_value(artist)
        });
        let title_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("info-item-title"))
                .default_value(title)
        });
        let _input_events = [&artist_input, &title_input]
            .map(|input| {
                cx.subscribe_in(
                    input,
                    window,
                    |this, _, event: &InputEvent, _, cx| match event {
                        InputEvent::Change => this.search_soon(true, cx),
                        InputEvent::PressEnter { .. } => this.search_soon(false, cx),
                        _ => {}
                    },
                )
            })
            .into_iter()
            .collect::<Vec<_>>();
        let _backdrop_changed = cx.observe(&now_art, |_, _, cx| cx.notify());
        let mut this = TagMatch {
            library,
            sink,
            key: key.clone(),
            line: line.into(),
            artist_input,
            title_input,
            album,
            duration_secs,
            can_identify: providers::acoustid_available() && key.sub == 0,
            search_task: None,
            current: vec![String::new(); FIELDS.len()],
            phase: Phase::Searching,
            selected: None,
            armed: vec![false; FIELDS.len()],
            saving: false,
            error: None,
            scroll: ScrollHandle::new(),
            now_art,
            backdrop: WindowBackdrop::default(),
            _input_events,
            _backdrop_changed,
        };
        this.read_current(cx);
        this.search_soon(false, cx);
        this
    }

    fn query(&self, cx: &App) -> TrackQuery {
        TrackQuery {
            artist: self.artist_input.read(cx).value().trim().to_string(),
            title: self.title_input.read(cx).value().trim().to_string(),
            album: self.album.clone(),
            duration_secs: self.duration_secs,
        }
    }

    /// A file that won't read leaves the current values empty.
    fn read_current(&self, cx: &mut Context<Self>) {
        let path = self.key.path.clone();
        cx.spawn(async move |this, cx| {
            let read = cx
                .background_executor()
                .spawn(async move { writer::read(&path) })
                .await;
            this.update(cx, |this, cx| {
                if let Ok(fields) = read {
                    for (i, (field, _, _)) in FIELDS.iter().enumerate() {
                        this.current[i] = fields
                            .iter()
                            .find(|(f, _)| f == field)
                            .map(|(_, v)| v.clone())
                            .unwrap_or_default();
                    }
                    this.rearm();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn search_soon(&mut self, debounce: bool, cx: &mut Context<Self>) {
        let query = self.query(cx);
        self.phase = Phase::Searching;
        cx.notify();
        self.search_task = Some(cx.spawn(async move |this, cx| {
            if debounce {
                cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            }
            let result = cx
                .background_executor()
                .spawn(async move { providers::search_metadata(&query) })
                .await;
            this.update(cx, |this, cx| this.apply_results(result, cx))
                .ok();
        }));
    }

    /// Fingerprint the file and ask AcoustID which recording it is. Stored in
    /// `search_task` so it cancels a pending debounce instead of racing it.
    /// A container with no length falls back to the library's duration.
    fn identify(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx);
        let path = self.key.path.clone();
        let fallback = self
            .duration_secs
            .map(|secs| secs.round().max(0.0) as u32)
            .filter(|&secs| secs > 0);
        let no_duration = rox_i18n::t!("tags-matcher-identify-no-duration").to_string();
        self.phase = Phase::Searching;
        cx.notify();
        self.search_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let fingerprint = rox_playback::fingerprint::compute(&path, || true)?;
                    let Some(duration_secs) = fingerprint.duration_secs.or(fallback) else {
                        return Err(no_duration);
                    };
                    // Never log the fingerprint itself.
                    log::debug!("acoustid identify: {duration_secs}s");
                    providers::identify(&fingerprint.encoded, duration_secs, &query)
                })
                .await;
            this.update(cx, |this, cx| this.apply_results(result, cx))
                .ok();
        }));
    }

    fn apply_results(
        &mut self,
        result: Result<Vec<MetadataCandidate>, String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(found) => {
                self.selected = (!found.is_empty()).then_some(0);
                self.phase = Phase::Ready(found);
                self.rearm();
            }
            Err(e) => {
                log::warn!("metadata search: {e}");
                self.phase = Phase::Failed(rox_i18n::t!("tags-matcher-search-failed", error = e));
            }
        }
        cx.notify();
    }

    fn rearm(&mut self) {
        let Phase::Ready(found) = &self.phase else {
            return;
        };
        let Some(candidate) = self.selected.and_then(|ix| found.get(ix)) else {
            return;
        };
        for (i, (_, _, pull)) in FIELDS.iter().enumerate() {
            let fetched = pull(candidate);
            self.armed[i] = !fetched.is_empty() && fetched != self.current[i];
        }
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Phase::Ready(found) = &self.phase else {
            return;
        };
        let Some(candidate) = self.selected.and_then(|ix| found.get(ix)) else {
            return;
        };
        let mut fields: Vec<(Field, String)> = Vec::new();
        for (i, (field, _, pull)) in FIELDS.iter().enumerate() {
            if !self.armed[i] {
                continue;
            }
            let value = pull(candidate);
            if value == self.current[i] {
                continue;
            }
            fields.push((field.clone(), value));
        }
        if fields.is_empty() {
            window.remove_window();
            return;
        }
        match &self.sink {
            Sink::Fill {
                editor,
                window: editor_window,
                track,
            } => {
                let editor = editor.clone();
                let track = *track;
                editor_window
                    .update(cx, |_, editor_win, cx| {
                        editor
                            .update(cx, |editor, cx| {
                                editor.fill_fields(track, &fields, editor_win, cx)
                            })
                            .ok();
                    })
                    .ok();
                window.remove_window();
            }
            Sink::Commit => {
                let changes = fields
                    .into_iter()
                    .map(|(field, value)| Change {
                        field,
                        value: (!value.is_empty()).then_some(value),
                    })
                    .collect();
                let edit = Edit {
                    path: self.key.path.clone(),
                    changes,
                    pictures: Vec::new(),
                };
                let sub = self.key.sub;
                self.saving = true;
                self.error = None;
                cx.notify();
                let library = self.library.clone();
                cx.spawn_in(window, async move |this, cx| {
                    let (edit, result) = cx
                        .background_executor()
                        .spawn(async move {
                            // Through the key, so a cue track's pick goes to the library instead of
                            // stamping the shared image.
                            let result =
                                writer::commit_key(&edit.path, sub, &edit.changes, &edit.pictures);
                            (edit, result)
                        })
                        .await;
                    this.update_in(cx, |this, window, cx| match Some(result) {
                        Some(Ok(())) => {
                            library
                                .update(cx, |library, cx| library.apply_edits(&[edit], &[sub], cx));
                            window.remove_window();
                        }
                        Some(Err(e)) => {
                            this.saving = false;
                            this.error = Some(e.into());
                            cx.notify();
                        }
                        None => {
                            this.saving = false;
                            cx.notify();
                        }
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    fn candidate_list(&self, found: &[MetadataCandidate], cx: &mut Context<Self>) -> Div {
        let mut body = div().flex().flex_col().gap(tokens::SPACE_XS);
        for (ix, candidate) in found.iter().enumerate() {
            let selected = self.selected == Some(ix);
            let mut sub = vec![candidate.album.clone()];
            if !candidate.year.is_empty() {
                sub.push(candidate.year.clone());
            }
            if let Some(secs) = candidate.duration_secs {
                sub.push(fmt_time(secs));
            }
            sub.retain(|s| !s.is_empty());
            body = body.child(
                div()
                    .id(("candidate", ix))
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_XS)
                    .p(tokens::SPACE_SM)
                    .rounded(tokens::RADIUS)
                    .border_1()
                    .border_color(if selected {
                        palette::accent()
                    } else {
                        palette::border()
                    })
                    .when(selected, |d| d.bg(palette::bg_control_active()))
                    .cursor_pointer()
                    .hover(|d| d.bg(palette::bg_menu_hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(ix);
                        this.rearm();
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(tokens::SPACE_SM)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(palette::text_bright())
                                    .child(SharedString::from(candidate.title.clone())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(palette::text_muted())
                                    .child(candidate.provider),
                            )
                            .child(confidence_badge(candidate.confidence)),
                    )
                    .when(!sub.is_empty(), |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .truncate()
                                .child(SharedString::from(sub.join("  "))),
                        )
                    })
                    .child(confidence_bar(candidate.confidence)),
            );
        }
        body
    }

    fn compare(&self, candidate: &MetadataCandidate, cx: &mut Context<Self>) -> Div {
        let mut rows = div().flex().flex_col().gap(tokens::SPACE_XS);
        for (i, (_, label, pull)) in FIELDS.iter().enumerate() {
            let current = self.current[i].clone();
            let fetched = pull(candidate);
            let changes = !fetched.is_empty() && fetched != current;
            let armed = self.armed[i] && changes;
            let row = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_SM)
                .py(tokens::SPACE_XS)
                .border_b_1()
                .border_color(palette::border())
                .child(
                    div()
                        .w(px(84.))
                        .flex_none()
                        .text_color(palette::text_muted())
                        .child(*label),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(palette::text_muted())
                        .child(value_or_dash(&current)),
                )
                .child(
                    div()
                        .w(px(14.))
                        .flex_none()
                        .text_color(palette::text_faint())
                        .child("→"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(if changes {
                            palette::text_bright()
                        } else {
                            palette::text_faint()
                        })
                        .child(value_or_dash(&fetched)),
                )
                .child(
                    div()
                        .w(px(48.))
                        .flex_none()
                        .flex()
                        .justify_end()
                        .when(changes, |d| {
                            d.child(settings_ui::icon_button(
                                if armed { icons::CHECK } else { icons::CLOSE },
                                false,
                                cx.listener(move |this, _, _, cx| {
                                    this.armed[i] = !this.armed[i];
                                    cx.notify();
                                }),
                            ))
                        }),
                );
            rows = rows.child(row);
        }
        rows
    }
}

/// Off the projection, so the score doesn't depend on what's playing.
fn duration_secs_for(library: &Entity<Library>, id: i64, cx: &App) -> Option<f64> {
    let library = library.read(cx);
    let projection = library.projection()?;
    let row = (0..projection.len() as u32)
        .find(|&row| projection.db_id[row as usize] == id && !projection.is_dead(row))?;
    let ms = projection.resolve(row).duration_ms;
    (ms > 0).then(|| ms as f64 / 1000.0)
}

fn value_or_dash(value: &str) -> SharedString {
    if value.is_empty() {
        SharedString::from("-")
    } else {
        SharedString::from(value.to_string())
    }
}

impl Render for TagMatch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let can_apply = self.blocker().is_none();
        let count = match &self.phase {
            Phase::Ready(found) if !found.is_empty() => Some(
                div()
                    .text_xs()
                    .text_color(palette::text())
                    .child(rox_i18n::t!(
                        "tags-matcher-match-count",
                        count = found.len() as u64
                    ))
                    .into_any_element(),
            ),
            _ => None,
        };

        let content = match &self.phase {
            Phase::Searching => note(rox_i18n::t!("tags-matcher-searching")),
            Phase::Failed(e) => crate::console_window::notice(e.clone()),
            Phase::Ready(found) if found.is_empty() => {
                note(rox_i18n::t!("tags-matcher-no-matches"))
            }
            Phase::Ready(found) => {
                let compare = match self.selected.and_then(|ix| found.get(ix)) {
                    Some(candidate) => div()
                        .id("compare")
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .child(self.compare(candidate, cx))
                        .into_any_element(),
                    None => div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(palette::text_faint())
                        .child(rox_i18n::t!("tags-matcher-pick-match"))
                        .into_any_element(),
                };
                div()
                    .flex()
                    .flex_row()
                    .gap(tokens::SPACE_MD)
                    .h_full()
                    .child(
                        div()
                            .id("candidate-list")
                            .w(px(280.))
                            .flex_none()
                            .h_full()
                            .overflow_y_scroll()
                            .child(self.candidate_list(found, cx)),
                    )
                    .child(compare)
            }
        };

        div()
            .size_full()
            .flex()
            .flex_col()
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
                    .gap(SECTION_GAP)
                    .p(tokens::SPACE_MD)
                    .bg(palette::bg_elevated())
                    .child(section(
                        rox_i18n::t!("query-search"),
                        None,
                        self.search_fields(cx),
                    ))
                    .when_some(self.error.clone(), |d, error| {
                        d.child(div().text_color(palette::text_muted()).child(error))
                    })
                    .child(
                        section(
                            rox_i18n::t!("matcher-section-matches"),
                            count,
                            div().flex_1().min_h_0().child(content),
                        )
                        .flex_1()
                        .min_h_0(),
                    ),
            )
            .child(self.footer(can_apply, cx))
    }
}

impl TagMatch {
    /// Ordered the way a lookup clears them, so the footer names the next step.
    fn blocker(&self) -> Option<SharedString> {
        if !matches!(self.phase, Phase::Ready(ref f) if !f.is_empty()) {
            return Some(match self.phase {
                Phase::Searching => "Searching...".into(),
                _ => rox_i18n::t!("tags-matcher-blocked-no-match"),
            });
        }
        if self.selected.is_none() {
            return Some(rox_i18n::t!("tags-matcher-blocked-pick"));
        }
        if !self.armed.iter().any(|&a| a) {
            return Some(rox_i18n::t!("tags-matcher-blocked-arm"));
        }
        if self.saving {
            return Some(rox_i18n::t!("tags-matcher-blocked-writing"));
        }
        None
    }

    /// No enter shortcut: the query boxes own the key as "search now", and a
    /// window binding would apply against results about to be replaced.
    fn footer(&self, can_apply: bool, cx: &mut Context<Self>) -> Div {
        let blocker = self.blocker();
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
            .child(match blocker {
                Some(reason) => div()
                    .text_xs()
                    .text_color(palette::tone_warn())
                    .child(reason),
                None => div(),
            })
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(settings_ui::small_button(
                        "Apply",
                        icons::CHECK,
                        !can_apply,
                        cx.listener(|this, _, window, cx| this.apply(window, cx)),
                    ))
                    .child(settings_ui::small_button(
                        rox_i18n::t!("settings-common-cancel"),
                        icons::CLOSE,
                        self.saving,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    )),
            )
    }

    fn search_fields(&self, cx: &mut Context<Self>) -> Div {
        let field = |label: SharedString, input: &Entity<InputState>| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(tokens::SPACE_XS)
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(label),
                )
                .child(Input::new(input).small())
        };
        // A fingerprint covers the whole file, so on a cue image it would identify
        // the disc, not the subsong. Settled at open in `can_identify`.
        let can_identify = self.can_identify;
        let busy = self.saving || matches!(self.phase, Phase::Searching);
        div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_faint())
                    .truncate()
                    .child(rox_i18n::t!(
                        "tags-matcher-tagging",
                        track = self.line.to_string()
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_end()
                    .gap(tokens::SPACE_SM)
                    .child(field(rox_i18n::t!("head-piece-artist"), &self.artist_input))
                    .child(field(rox_i18n::t!("info-item-title"), &self.title_input))
                    .when(can_identify, |d| {
                        d.child(settings_ui::small_button(
                            rox_i18n::t!("tags-matcher-identify"),
                            icons::AUDIO_WAVEFORM,
                            busy,
                            cx.listener(|this, _, _, cx| this.identify(cx)),
                        ))
                    }),
            )
    }
}
