//! The lyrics match window: an online search verified before it writes.
//! The artist and title boxes start from the track and search again as
//! they're edited, for a song the providers know under another name. Apply
//! saves the picked sheet through the editor's save path to the
//! Providers page's destination, or the store for a track with no file.
//!
//! Keyed by subject rather than path: a station's words belong to the song
//! it announced, not the stream URL.

use gpui::{
    App, Bounds, Context, Div, Entity, Global, ScrollHandle, SharedString, Subscription, Task,
    Window, WindowHandle, div, prelude::*, px, size,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{Root, Sizable as _};

use rox_core::fmt::fmt_ms;
use rox_library::lyrics::{self, Subject};

use crate::matching::{
    Phase, WindowRegistry, confidence_badge, confidence_bar, note, open_or_focus,
};
use rox_core::settings::lyrics_dir;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_net::providers::{self, LyricsCandidate, TrackQuery};
use rox_panel_api::panel::AppState;
use rox_panel_kit::ui::{self as settings_ui, SECTION_GAP, section};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::lyrics::{LyricsTarget, PluginLyrics, save_target};
use rox_services::player::fmt_time;

const DEFAULT_SIZE: (f32, f32) = (720., 560.);

const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(350);

#[derive(Default)]
struct OpenMatchers(Vec<(Subject, WindowHandle<Root>)>);

impl Global for OpenMatchers {}

impl WindowRegistry for OpenMatchers {
    type Key = Subject;
    fn entries(&mut self) -> &mut Vec<(Subject, WindowHandle<Root>)> {
        &mut self.0
    }
}

pub fn open(state: AppState, target: LyricsTarget, cx: &mut App) {
    open_or_focus::<OpenMatchers>(
        target.subject.clone(),
        move |cx| {
            let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
            rox_panel_api::panel::open_child_window(
                cx,
                rox_i18n::t!("lyrics-matcher-window-title"),
                bounds,
                Some(settings_ui::MIN_SIZE),
                move |window, cx| cx.new(|cx| LyricsMatch::new(state, target, window, cx)),
            )
        },
        cx,
    );
}

struct LyricsMatch {
    subject: Subject,
    duration_ms: u32,
    /// The track's own query, for the album and duration the boxes don't show.
    base: TrackQuery,
    artist_input: Entity<InputState>,
    title_input: Entity<InputState>,
    plugin: Option<PluginLyrics>,
    /// The plugin's sheet once it answered. It's keyed by the track, not the
    /// words in the boxes, so it's asked once and leads every search after.
    mine: Option<Option<LyricsCandidate>>,
    /// Replacing it cancels the last timer and any request in flight.
    search_task: Option<Task<()>>,
    phase: Phase<LyricsCandidate>,
    selected: Option<usize>,
    saving: bool,
    error: Option<SharedString>,
    preview_scroll: ScrollHandle,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    _input_events: Vec<Subscription>,
    _backdrop_changed: Subscription,
}

impl LyricsMatch {
    fn new(
        state: AppState,
        target: LyricsTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let LyricsTarget {
            subject,
            query,
            plugin,
        } = target;
        let duration_ms = query
            .duration_secs
            .map(|secs| (secs * 1000.0) as u32)
            .unwrap_or(0);

        let artist_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("head-piece-artist"))
                .default_value(query.artist.clone())
        });
        let title_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(rox_i18n::t!("info-item-title"))
                .default_value(query.title.clone())
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
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());

        let mut this = LyricsMatch {
            subject,
            duration_ms,
            base: query,
            artist_input,
            title_input,
            plugin,
            mine: None,
            search_task: None,
            phase: Phase::Searching,
            selected: None,
            saving: false,
            error: None,
            preview_scroll: ScrollHandle::new(),
            now_art: state.now_art,
            backdrop: WindowBackdrop::default(),
            _input_events,
            _backdrop_changed,
        };
        this.search_soon(false, cx);
        this
    }

    fn query(&self, cx: &App) -> TrackQuery {
        TrackQuery {
            artist: self.artist_input.read(cx).value().trim().to_string(),
            title: self.title_input.read(cx).value().trim().to_string(),
            ..self.base.clone()
        }
    }

    fn search_soon(&mut self, debounce: bool, cx: &mut Context<Self>) {
        let query = self.query(cx);
        self.selected = None;
        self.preview_scroll.set_offset(Default::default());

        // Nothing to match on: say so rather than search for an empty result.
        // A plugin answers by the track itself, so it still gets asked.
        if self.plugin.is_none() && (query.artist.is_empty() || query.title.is_empty()) {
            self.search_task = None;
            self.phase = Phase::Failed(rox_i18n::t!("lyrics-matcher-no-query"));
            cx.notify();
            return;
        }

        self.phase = Phase::Searching;
        cx.notify();

        let plugin = match self.mine {
            Some(_) => None,
            None => self.plugin.clone(),
        };
        let kept = self.mine.clone();
        self.search_task = Some(cx.spawn(async move |this, cx| {
            if debounce {
                cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            }

            let (mine, online) = cx
                .background_executor()
                .spawn(async move {
                    let mine = match &plugin {
                        Some(plugin) => ask_plugin(plugin, &query),
                        None => kept.flatten(),
                    };
                    (mine, search_online(&query))
                })
                .await;

            this.update(cx, |this, cx| {
                this.mine = Some(mine.clone());
                this.fill(merge(mine, online));
                cx.notify();
            })
            .ok();
        }));
    }

    fn fill(&mut self, result: Result<Vec<LyricsCandidate>, String>) {
        match result {
            Ok(found) => {
                self.selected = (!found.is_empty()).then_some(0);
                self.phase = Phase::Ready(found);
            }
            Err(e) => {
                log::warn!("lyrics search: {e}");
                self.phase = Phase::Failed(rox_i18n::t!("lyrics-matcher-search-failed", error = e));
            }
        }
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Phase::Ready(found) = &self.phase else {
            return;
        };
        let Some(text) = self
            .selected
            .and_then(|ix| found.get(ix))
            .map(|c| c.text.clone())
        else {
            return;
        };
        let subject = self.subject.clone();
        self.saving = true;
        self.error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let saved = cx
                .background_executor()
                .spawn({
                    let subject = subject.clone();
                    async move {
                        let target = save_target(&subject);

                        lyrics::save(&subject, &target, &text, Some(&lyrics_dir()))
                    }
                })
                .await;
            this.update_in(cx, |this, window, cx| {
                match saved {
                    Ok(()) => {
                        // Lyrics aren't in the projection, so every panel
                        // needs a poke to re-read.
                        crate::lyrics::saved(&subject, cx);
                        window.remove_window();
                    }
                    Err(e) => {
                        this.saving = false;
                        this.error = Some(e.into());
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

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
            .flex_row()
            .gap(tokens::SPACE_SM)
            .child(field(rox_i18n::t!("head-piece-artist"), &self.artist_input))
            .child(field(rox_i18n::t!("info-item-title"), &self.title_input))
    }

    fn candidate_list(&self, found: &[LyricsCandidate], cx: &mut Context<Self>) -> Div {
        let mut body = div().flex().flex_col().gap(tokens::SPACE_XS);
        for (ix, candidate) in found.iter().enumerate() {
            let selected = self.selected == Some(ix);
            let subtitle = {
                let mut parts = vec![candidate.artist.clone()];
                if !candidate.album.is_empty() {
                    parts.push(candidate.album.clone());
                }
                if let Some(secs) = candidate.duration_secs {
                    parts.push(fmt_time(secs));
                }
                parts.retain(|p| !p.is_empty());
                parts.join("  ")
            };
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
                        this.preview_scroll.set_offset(Default::default());
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
                                    .child(if candidate.synced {
                                        rox_i18n::t!(
                                            "lyrics-matcher-synced-tag",
                                            provider = candidate.provider.as_ref()
                                        )
                                    } else {
                                        SharedString::from(candidate.provider.to_string())
                                    }),
                            )
                            .child(confidence_badge(candidate.confidence)),
                    )
                    .when(!subtitle.is_empty(), |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(palette::text_muted())
                                .truncate()
                                .child(SharedString::from(subtitle)),
                        )
                    })
                    .child(confidence_bar(candidate.confidence)),
            );
        }
        body
    }

    /// Raw text, timing tags and all: exactly what a save would write.
    fn preview(&self, found: &[LyricsCandidate]) -> Div {
        let text = self
            .selected
            .and_then(|ix| found.get(ix))
            .map(|c| c.text.clone());
        let body = match text {
            Some(text) => div()
                .id("lyrics-preview")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.preview_scroll)
                .p(tokens::SPACE_MD)
                .text_color(palette::text())
                .children(text.lines().map(|line| {
                    if line.trim().is_empty() {
                        div().h(px(10.))
                    } else {
                        div().child(SharedString::from(line.to_string()))
                    }
                }))
                .into_any_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(palette::text_faint())
                .child(rox_i18n::t!("lyrics-matcher-pick-preview"))
                .into_any_element(),
        };
        div()
            .flex_1()
            .min_w_0()
            .rounded(tokens::RADIUS)
            .border_1()
            .border_color(palette::border())
            .bg(palette::bg_root())
            .overflow_hidden()
            .child(body)
    }

    /// Ordered the way a search clears them, so the footer names the next step.
    fn blocker(&self) -> Option<SharedString> {
        if !matches!(self.phase, Phase::Ready(ref f) if !f.is_empty()) {
            return Some(match self.phase {
                Phase::Searching => "Searching...".into(),
                _ => rox_i18n::t!("lyrics-matcher-blocked-no-match"),
            });
        }
        if self.selected.is_none() {
            return Some(rox_i18n::t!("lyrics-matcher-blocked-pick"));
        }
        if self.saving {
            return Some(rox_i18n::t!("lyrics-matcher-blocked-saving"));
        }
        None
    }

    /// No Enter binding: the query boxes use Enter to search now, and a window
    /// binding would apply off results that search is about to replace.
    fn footer(&self, can_apply: bool, cx: &mut Context<Self>) -> Div {
        let hint = match self.blocker() {
            Some(reason) => div()
                .text_xs()
                .text_color(palette::tone_warn())
                .child(reason),
            None => div(),
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
}

impl Render for LyricsMatch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let can_apply = self.blocker().is_none();
        let count = match &self.phase {
            Phase::Ready(found) if !found.is_empty() => Some(
                div()
                    .text_xs()
                    .text_color(palette::text())
                    .child(rox_i18n::t!(
                        "lyrics-matcher-match-count",
                        count = found.len() as u64
                    ))
                    .into_any_element(),
            ),
            _ => None,
        };

        // The track's own length, to hold each match's against.
        let duration = (self.duration_ms > 0).then(|| {
            div()
                .text_xs()
                .text_color(palette::text_muted())
                .child(fmt_ms(self.duration_ms))
                .into_any_element()
        });

        let content = match &self.phase {
            Phase::Searching => note("Searching..."),
            Phase::Failed(e) => crate::console_window::notice(e.clone()),
            Phase::Ready(found) if found.is_empty() => note("No matches found"),
            Phase::Ready(found) => div()
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
                .child(self.preview(found)),
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
                    .bg(palette::bg_elevated())
                    .gap(SECTION_GAP)
                    .p(tokens::SPACE_MD)
                    .child(section(
                        rox_i18n::t!("query-search"),
                        duration,
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

/// A plugin that fails only costs its row: the providers still answer.
/// Blocking.
fn ask_plugin(plugin: &PluginLyrics, query: &TrackQuery) -> Option<LyricsCandidate> {
    plugin.ask(query).unwrap_or_else(|e| {
        log::warn!("lyrics from {}: {e}", plugin.source);
        None
    })
}

/// Blocking.
fn search_online(query: &TrackQuery) -> Result<Vec<LyricsCandidate>, String> {
    match query.artist.is_empty() || query.title.is_empty() {
        true => Ok(Vec::new()),
        false => providers::search_lyrics(query),
    }
}

/// The plugin's own sheet leads, since it's for this very track.
fn merge(
    mine: Option<LyricsCandidate>,
    online: Result<Vec<LyricsCandidate>, String>,
) -> Result<Vec<LyricsCandidate>, String> {
    match (mine, online) {
        (Some(mine), Ok(mut found)) => {
            found.insert(0, mine);
            Ok(found)
        }

        (Some(mine), Err(e)) => {
            log::warn!("lyrics search: {e}");
            Ok(vec![mine])
        }

        (None, online) => online,
    }
}
