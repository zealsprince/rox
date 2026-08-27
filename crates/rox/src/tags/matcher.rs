//! The metadata match window: one OS window opened on a track with no
//! good tags, so an online lookup is verified field by field before it
//! writes. It reads the track's current tags, searches the providers off
//! the UI thread, and lists the candidates best first with a confidence
//! bar. The selected candidate shows as a compare table, each field's
//! current value beside the fetched one, and a per-field toggle arms the
//! ones to take. Apply writes only the armed fields through the same
//! atomic commit the tag editor uses, then closes; the library reload
//! refreshes every panel. Nothing is written until Apply.
//!
//! One window per track path and opening editor, registered like the
//! cover editor.

use gpui::{
    div, prelude::*, px, size, AnyWindowHandle, App, Bounds, Context, Div, Entity, EntityId,
    Global, ScrollHandle, SharedString, Subscription, Task, WeakEntity, Window, WindowHandle,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Root, Sizable as _};

use rox_library::cue::TrackKey;
use rox_library::writer::{self, Change, Edit, Field};

use crate::matching::{
    confidence_badge, confidence_bar, note, open_or_focus, Phase, WindowRegistry,
};
use crate::tags::editor::TagEditor;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_net::providers::{self, MetadataCandidate, TrackQuery};
use rox_panel_kit::ui::{self as settings_ui, section, SECTION_GAP};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::catalog::Library;
use rox_services::player::fmt_time;

/// What Apply does with the picked fields: write them to the file, or hand
/// them to a tag editor's form. The fill keeps the editor the single
/// writer, so the compare never writes tags behind an open editor's back
/// and leaves its baselines stale.
enum Sink {
    /// The metadata panel's lookup: commit straight to the track.
    Commit,
    /// The tag editor's lookup: fill it, the editor saves. The window
    /// handle is the editor's own, needed to set its inputs from this
    /// window; both weak, so a closed editor drops the fill. `track`
    /// names which of its tracks this ran on, so a fill from one of the
    /// table's rows goes into that row rather than over the batch.
    Fill {
        editor: WeakEntity<TagEditor>,
        window: AnyWindowHandle,
        track: usize,
    },
}

/// The fields the compare shows, in tag-sheet order: the writer field, its
/// label, and how to pull the value off a candidate. Rating and lyrics stay
/// out, since a release lookup doesn't return them.
type Pull = fn(&MetadataCandidate) -> String;
const FIELDS: &[(Field, &str, Pull)] = &[
    (Field::Title, "Title", |c| c.title.clone()),
    (Field::Artist, "Artist", |c| c.artist.clone()),
    (Field::AlbumArtist, "Album Artist", |c| {
        c.album_artist.clone()
    }),
    (Field::Album, "Album", |c| c.album.clone()),
    (Field::Year, "Year", |c| c.year.clone()),
    (Field::TrackNo, "Track", |c| c.track_no.clone()),
    (Field::DiscNo, "Disc", |c| c.disc_no.clone()),
];

/// The default window size: room for the candidate list beside the compare
/// table without either crowding.
const DEFAULT_SIZE: (f32, f32) = (760., 560.);

/// How long the query rests before an edit fires a search, so a burst of
/// typing spends one request, not one a keystroke.
const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(350);

/// The registry key: the track, plus the opening editor for fills. A fill's
/// window binds its Apply to the editor that opened it, so two editors on the
/// same track need their own windows; the panel's commit lookup has no
/// editor and shares one window per track. Keyed on the whole track, so two
/// tracks of one cue image get a window each.
type MatchKey = (TrackKey, Option<EntityId>);

/// The open match windows, keyed so a second request for the same track
/// (and editor, for fills) focuses the first.
#[derive(Default)]
struct OpenMatchers(Vec<(MatchKey, WindowHandle<Root>)>);

impl Global for OpenMatchers {}

impl WindowRegistry for OpenMatchers {
    type Key = MatchKey;
    fn entries(&mut self) -> &mut Vec<(MatchKey, WindowHandle<Root>)> {
        &mut self.0
    }
}

/// Open a metadata compare that writes straight to the track on apply,
/// the metadata panel's lookup.
pub fn open(library: Entity<Library>, now_art: Entity<NowPlayingArt>, key: TrackKey, cx: &mut App) {
    open_with(library, now_art, key, Sink::Commit, cx);
}

/// Open a metadata compare that fills a tag editor on apply rather than
/// writing, so the editor stays the one writer. The editor and its window
/// are both what the fill sets, and both weak, so a closed editor no-ops.
/// `track` is the editor's index for the track this ran on.
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

/// Open a metadata compare on `key`, or focus the one already on it.
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
    /// What Apply does with the picked fields.
    sink: Sink,
    /// The track the tags write back to.
    key: TrackKey,
    /// The track as the header shows it.
    line: SharedString,
    /// The editable query fields, seeded from the track's tags: what the
    /// search sends and what the confidence scores against, so fixing a
    /// wrong tag both finds and ranks the right release.
    artist_input: Entity<InputState>,
    title_input: Entity<InputState>,
    /// The album and duration the query keeps from the tags, not editable
    /// here: they steer the best-release pick and the score without a box
    /// of their own.
    album: String,
    duration_secs: Option<f64>,
    /// The pending debounced search; replacing it cancels the last timer
    /// and any in-flight request, the workspace's save-debounce idiom.
    search_task: Option<Task<()>>,
    /// The current tag values, one per [`FIELDS`], read off the file so
    /// the compare shows what a write would replace.
    current: Vec<String>,
    phase: Phase<MetadataCandidate>,
    /// The highlighted candidate, an index into the ready list.
    selected: Option<usize>,
    /// Which fields to write, one per [`FIELDS`], reset when the selection
    /// changes: on where the fetched value is non-empty and differs.
    armed: Vec<bool>,
    /// A commit is in flight; the buttons hold still until it finishes.
    saving: bool,
    /// A failed read or commit, shown inline over the buttons.
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
        // The query fields seed from the tags and drive both the search and
        // the score, so an edit finds and ranks the right release.
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
                        // Debounce the typing; Enter searches at once.
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

    /// The query as the boxes and the kept album and duration stand now.
    fn query(&self, cx: &App) -> TrackQuery {
        TrackQuery {
            artist: self.artist_input.read(cx).value().trim().to_string(),
            title: self.title_input.read(cx).value().trim().to_string(),
            album: self.album.clone(),
            duration_secs: self.duration_secs,
        }
    }

    /// Read the track's current tags off the UI thread and fold them into
    /// the compare's left column. A file that will not read leaves the
    /// current values empty; the compare still shows what a write sets.
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

    /// Search the providers for the current query and fill the list when
    /// it returns. With `debounce`, wait out a beat of quiet first so a burst
    /// of typing fires one request; storing the task cancels the previous
    /// timer and any request still in flight. Enter skips the wait.
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

    /// Fold a finished search into the list, the top score pre-selected,
    /// or leave the failure for the header to show.
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

    /// Arm every field the selected candidate would change: a non-empty
    /// fetched value that differs from the current tag. Run whenever the
    /// selection or the baselines move, so the default is "take what is
    /// new" and the user pares back from there.
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

    /// Apply the armed fields: the armed values that actually change,
    /// gathered once, then either committed to the track or handed to a
    /// tag editor's form depending on the sink.
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
            // The tag editor's lookup: fill its form on its own window, so
            // the editor's normal save writes them and its baselines never
            // go stale behind an unseen commit. No file write here.
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
            // The metadata panel's lookup: commit off the UI thread, reload
            // the library so every panel refreshes, and close; a failure
            // keeps the window open with the error, the file untouched.
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
                            // Through the key, so a cue track's pick is
                            // written to the library instead of stamping
                            // the shared image.
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

    /// The candidate list: one row each, best first, the album and year so
    /// releases tell apart, the confidence as a bar and a percent. Clicking
    /// selects and re-arms the compare.
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
                                // The service the row came from, so a later
                                // second provider tells its matches apart.
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

    /// The compare: one row per field, the current tag beside the fetched
    /// value with a toggle to arm it. A field the candidate doesn't have,
    /// or already matches, shows dimmed and inert, since there's nothing
    /// to take.
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

/// The track's duration in seconds off the projection, resolved from its
/// id, so the score doesn't depend on the track being the one playing.
fn duration_secs_for(library: &Entity<Library>, id: i64, cx: &App) -> Option<f64> {
    let library = library.read(cx);
    let projection = library.projection()?;
    let row = projection.db_id.iter().position(|&db| db == id)?;
    let ms = projection.resolve(row as u32).duration_ms;
    (ms > 0).then(|| ms as f64 / 1000.0)
}

/// A tag value, or a dash where it's empty, so an empty cell reads as
/// "nothing here" rather than a gap.
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
            // The backdrop paints first, under the page, so translucent
            // surfaces back with the playing track's art like every window.
            .children(self.backdrop.layer(&self.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(SECTION_GAP)
                    .p(tokens::SPACE_MD)
                    // The body's own surface, a second elevated layer over
                    // the window's, the same as the settings page. The
                    // backdrop reads through two layers everywhere.
                    .bg(palette::bg_elevated())
                    .child(section(
                        rox_i18n::t!("query-search"),
                        None,
                        self.search_fields(),
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
    /// What stands between the window and a write, when something does.
    /// The clauses run in the order a lookup clears them, so the footer
    /// names the one step that's actually next, and Apply is live exactly
    /// when nothing is left.
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

    /// The window's actions, and what's in their way. No enter shortcut
    /// here: the query boxes own the key as "search now", and a window
    /// binding would fire on the same press and apply against
    /// results the search is about to replace.
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
                        "Cancel",
                        icons::CLOSE,
                        self.saving,
                        cx.listener(|_, _, window, _| window.remove_window()),
                    )),
            )
    }

    /// The search area: the track being tagged for context, then the
    /// editable artist and title that drive the lookup. Editing either
    /// re-searches after a beat; Enter searches at once.
    fn search_fields(&self) -> Div {
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
                    .gap(tokens::SPACE_SM)
                    .child(field(rox_i18n::t!("head-piece-artist"), &self.artist_input))
                    .child(field(rox_i18n::t!("info-item-title"), &self.title_input)),
            )
    }
}
