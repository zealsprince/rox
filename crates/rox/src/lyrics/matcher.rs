//! The lyrics match window: an online search verified before it writes.
//! Apply saves the picked sheet through the editor's save path to the
//! Providers page's destination, or the store for a track with no file.
//!
//! Keyed by subject rather than path: a station's words belong to the song
//! it announced, not the stream URL.

use gpui::{
    App, Bounds, Context, Div, Entity, FocusHandle, Global, KeyBinding, ScrollHandle, SharedString,
    Subscription, Window, WindowHandle, actions, div, prelude::*, px, size,
};
use gpui_component::Root;

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
use rox_panel_kit::ui::{self as settings_ui, SECTION_GAP, Seg, kbd_line, section};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::lyrics::{LyricsTarget, save_target};
use rox_services::player::fmt_time;

const DEFAULT_SIZE: (f32, f32) = (720., 560.);

actions!(lyrics_match, [Apply]);

const CONTEXT: &str = "LyricsMatch";

/// On the window root: there are no fields here to take Enter first.
pub fn bindings() -> Vec<KeyBinding> {
    vec![KeyBinding::new("enter", Apply, Some(CONTEXT))]
}

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
    line: SharedString,
    duration_ms: u32,
    phase: Phase<LyricsCandidate>,
    selected: Option<usize>,
    saving: bool,
    error: Option<SharedString>,
    preview_scroll: ScrollHandle,
    /// Held so the Enter binding has a dispatch path; nothing else takes focus.
    focus: FocusHandle,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    _backdrop_changed: Subscription,
}

impl LyricsMatch {
    fn new(
        state: AppState,
        target: LyricsTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let LyricsTarget { subject, query } = target.clone();
        let duration_ms = query
            .duration_secs
            .map(|secs| (secs * 1000.0) as u32)
            .unwrap_or(0);
        let line = target.label();
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        let focus = cx.focus_handle();
        window.focus(&focus);
        let this = LyricsMatch {
            subject,
            line: line.into(),
            duration_ms,
            phase: Phase::Searching,
            selected: None,
            saving: false,
            error: None,
            preview_scroll: ScrollHandle::new(),
            focus,
            now_art: state.now_art,
            backdrop: WindowBackdrop::default(),
            _backdrop_changed,
        };
        // Nothing to match on: say so rather than search for an empty result.
        if query.artist.is_empty() || query.title.is_empty() {
            let mut this = this;
            this.phase = Phase::Failed(rox_i18n::t!("lyrics-matcher-no-query"));
            return this;
        }
        this.search(query, cx);
        this
    }

    fn search(&self, query: TrackQuery, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { providers::search_lyrics(&query) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(found) => {
                        this.selected = (!found.is_empty()).then_some(0);
                        this.phase = Phase::Ready(found);
                    }
                    Err(e) => {
                        log::warn!("lyrics search: {e}");
                        this.phase =
                            Phase::Failed(rox_i18n::t!("lyrics-matcher-search-failed", error = e));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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

    fn track_row(&self) -> Div {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_MD)
            .child(div().flex_1().min_w_0().truncate().child(self.line.clone()))
            .when(self.duration_ms > 0, |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_color(palette::text_muted())
                        .child(fmt_ms(self.duration_ms)),
                )
            })
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
                                            provider = candidate.provider
                                        )
                                    } else {
                                        SharedString::from(candidate.provider)
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

    fn footer(&self, can_apply: bool, cx: &mut Context<Self>) -> Div {
        let hint = match self.blocker() {
            Some(reason) => div()
                .text_xs()
                .text_color(palette::tone_warn())
                .child(reason)
                .into_any_element(),
            None => kbd_line([
                Seg::Text("Press".into()),
                Seg::Key("Enter".into()),
                Seg::Text("to apply".into()),
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
            .track_focus(&self.focus)
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
                    .bg(palette::bg_elevated())
                    .gap(SECTION_GAP)
                    .p(tokens::SPACE_MD)
                    .child(section(
                        rox_i18n::t!("menu-section-track"),
                        None,
                        self.track_row(),
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
