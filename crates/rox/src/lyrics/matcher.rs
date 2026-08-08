//! The lyrics match window: one OS window opened from the lyrics panel
//! when a track has no words, so an online search is verified before it
//! writes anything. It runs the providers' search off the UI thread,
//! lists the candidates best first with a confidence bar, and previews
//! the selected sheet on the right. Apply saves the picked candidate
//! through the same lyrics save the editor uses, honoring the Providers
//! page's tag/sidecar/store destination, then tells every lyrics panel to
//! re-read and closes. Nothing is written until Apply; closing walks away clean.
//!
//! One window per track path, registered like the cover editor, so asking
//! again focuses the open one instead of stacking a twin.

use std::path::PathBuf;

use gpui::{
    div, prelude::*, px, size, App, Bounds, Context, Div, Entity, Global, ScrollHandle,
    SharedString, Subscription, Window, WindowHandle,
};
use gpui_component::Root;

use rox_core::fmt::fmt_ms;
use rox_library::lyrics;

use crate::matching::{
    confidence_badge, confidence_bar, note, open_or_focus, Phase, WindowRegistry,
};
use rox_core::settings::lyrics_dir;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_net::providers::{self, LyricsCandidate, TrackQuery};
use rox_panel_api::panel::AppState;
use rox_panel_kit::ui::{self as settings_ui, section, SECTION_GAP};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::lyrics::{query_for, save_target};
use rox_services::player::fmt_time;

/// The default window size: room for the candidate list beside a preview
/// that reads a verse or two without scrolling.
const DEFAULT_SIZE: (f32, f32) = (720., 560.);

/// The open match windows, keyed by track path, so a second request for
/// the same track focuses the first - the cover editor's registry shape.
#[derive(Default)]
struct OpenMatchers(Vec<(PathBuf, WindowHandle<Root>)>);

impl Global for OpenMatchers {}

impl WindowRegistry for OpenMatchers {
    type Key = PathBuf;
    fn entries(&mut self) -> &mut Vec<(PathBuf, WindowHandle<Root>)> {
        &mut self.0
    }
}

/// Open a lyrics match window on `path`, or focus the one already on it. A
/// save broadcasts through [`crate::lyrics::saved`], so the window never
/// holds a panel of its own.
pub fn open(state: AppState, path: PathBuf, cx: &mut App) {
    open_or_focus::<OpenMatchers>(
        path.clone(),
        move |cx| {
            let bounds = Bounds::centered(None, size(px(DEFAULT_SIZE.0), px(DEFAULT_SIZE.1)), cx);
            rox_panel_api::panel::open_child_window(
                cx,
                "rox - Find Lyrics",
                bounds,
                Some(settings_ui::MIN_SIZE),
                move |window, cx| cx.new(|cx| LyricsMatch::new(state, path, window, cx)),
            )
        },
        cx,
    );
}

struct LyricsMatch {
    /// The track the words save back to.
    path: PathBuf,
    /// The track as the header shows it, and what the candidates scored
    /// against.
    line: SharedString,
    duration_ms: u32,
    phase: Phase<LyricsCandidate>,
    /// The highlighted candidate, an index into the ready list; the search
    /// pre-selects the top score.
    selected: Option<usize>,
    /// A save is in flight; the buttons hold still until it lands.
    saving: bool,
    /// A failed save, shown inline over the buttons.
    error: Option<SharedString>,
    /// The preview pane's scroll, so a long sheet reads on its own.
    preview_scroll: ScrollHandle,
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    _backdrop_changed: Subscription,
}

impl LyricsMatch {
    fn new(state: AppState, path: PathBuf, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        // The query is the track's library tags, and the duration comes off
        // the projection so it scores whether or not the track is playing.
        let query = query_for(&state.library, &path, cx);
        let duration_ms = query
            .duration_secs
            .map(|secs| (secs * 1000.0) as u32)
            .unwrap_or(0);
        let mut line = query.title.clone();
        if !query.artist.is_empty() {
            line = format!("{} - {}", query.title, query.artist);
        }
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        let this = LyricsMatch {
            path,
            line: line.into(),
            duration_ms,
            phase: Phase::Searching,
            selected: None,
            saving: false,
            error: None,
            preview_scroll: ScrollHandle::new(),
            now_art: state.now_art,
            backdrop: WindowBackdrop::default(),
            _backdrop_changed,
        };
        // A query with nothing to match on can only miss; say so instead of
        // a search that comes back empty for the wrong reason.
        if query.artist.is_empty() || query.title.is_empty() {
            let mut this = this;
            this.phase = Phase::Failed("This track has no artist and title to match on".into());
            return this;
        }
        this.search(query, cx);
        this
    }

    /// Run the providers' search off the UI thread and fill the list when
    /// it lands, the top score pre-selected.
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
                        this.phase = Phase::Failed(format!("Search failed: {e}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Save the selected candidate where the Providers page says, off the
    /// UI thread. Success re-reads the panels and closes; a failure keeps
    /// the window open with the error, the file untouched.
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
        let path = self.path.clone();
        self.saving = true;
        self.error = None;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let saved =
                cx.background_executor()
                    .spawn({
                        let path = path.clone();
                        async move {
                            lyrics::save(&path, &save_target(&path), &text, Some(&lyrics_dir()))
                        }
                    })
                    .await;
            this.update_in(cx, |this, window, cx| {
                match saved {
                    Ok(()) => {
                        // Panels cache lyrics off the projection, so every
                        // one of them needs a poke to re-read.
                        crate::lyrics::saved(&path, cx);
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

    /// The header: what we are matching against, so the candidate rows
    /// read as better or worse than the track in hand.
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

    /// The candidate list: one row each, best first, the confidence as a
    /// bar and a percent, the synced ones badged. Clicking selects; the
    /// preview follows.
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
                                // The service and, for a timed sheet, a
                                // synced tag, so the row says where the
                                // words came from and what shape they take.
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(palette::text_muted())
                                    .child(SharedString::from(if candidate.synced {
                                        format!("{}  synced", candidate.provider)
                                    } else {
                                        candidate.provider.to_string()
                                    })),
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

    /// The preview pane: the selected candidate's sheet as raw text, timing
    /// tags and all, so a verify sees exactly what a save would write.
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
                .child("Pick a match to preview")
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
}

impl Render for LyricsMatch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Apply is live only with a candidate picked and no save running.
        let can_apply = matches!(self.phase, Phase::Ready(ref f) if !f.is_empty())
            && self.selected.is_some()
            && !self.saving;
        let buttons = div()
            .flex()
            .flex_row()
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
            ))
            .into_any_element();

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
                    .child(section("Track", Some(buttons), self.track_row()))
                    .when_some(self.error.clone(), |d, error| {
                        d.child(div().text_color(palette::text_muted()).child(error))
                    })
                    .child(div().flex_1().min_h_0().child(content)),
            )
    }
}
