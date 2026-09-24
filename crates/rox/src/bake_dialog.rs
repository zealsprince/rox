//! The embed dialog: which of the three stored sources (lyrics, gain,
//! acoustic) to write into the files, and what each would come to. The save
//! settings only apply to the next write, so this is the catch-up.
//!
//! The counts are real, from a survey that reads every candidate's tags, so
//! the window opens counting. The run belongs to [`crate::bake`].

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    App, Bounds, Context, Div, Entity, Global, KeyBinding, SharedString, Stateful, Subscription,
    Window, WindowHandle, actions, div, prelude::*, px, size,
};
use gpui_component::Root;

use rox_core::settings::{LayoutSize, Settings, lyrics_dir};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_library::bake::{self, Candidate, Counts, Source};
use rox_panel_kit::ui::{self as settings_ui, Seg, kbd_line, section};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::catalog::Library;

// Aliased: `bake` would otherwise name both crates' modules.
use crate::bake as job;

const MIN: gpui::Size<gpui::Pixels> = gpui::Size {
    width: px(420.),
    height: px(280.),
};

const TICK: Duration = Duration::from_millis(250);

actions!(bake_dialog, [Embed]);

const CONTEXT: &str = "BakeDialog";

/// On the window root, so Enter embeds wherever focus is.
pub fn bindings() -> Vec<KeyBinding> {
    vec![KeyBinding::new("enter", Embed, Some(CONTEXT))]
}

/// One at a time: it works on the whole library.
struct OpenBake(WindowHandle<Root>);

impl Global for OpenBake {}

pub fn open(library: Entity<Library>, now_art: Entity<NowPlayingArt>, cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenBake>() {
        let handle = open.0;
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    let (width, height) = Settings::load()
        .windows
        .bake_dialog
        .filter(|s| s.width >= f32::from(MIN.width) && s.height >= f32::from(MIN.height))
        .map(|s| (s.width, s.height))
        .unwrap_or((520., 340.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let handle = rox_panel_api::panel::open_child_window(
        cx,
        rox_i18n::t!("bake-window-title"),
        bounds,
        Some(MIN),
        move |window, cx| cx.new(|cx| BakeDialog::new(library, now_art, window, cx)),
    );
    cx.set_global(OpenBake(handle));
}

pub struct BakeDialog {
    library: Entity<Library>,
    survey: Option<Arc<job::Survey>>,
    candidates: Vec<Candidate>,
    error: Option<SharedString>,
    /// In [`Source::ALL`] order.
    picked: [bool; 3],
    now_art: Entity<NowPlayingArt>,
    backdrop: WindowBackdrop,
    _backdrop_changed: Subscription,
}

impl BakeDialog {
    fn new(
        library: Entity<Library>,
        now_art: Entity<NowPlayingArt>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let _backdrop_changed = cx.observe(&now_art, |_, _, cx| cx.notify());
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            if let Some(this) = this.upgrade() {
                this.update(cx, |this, cx| {
                    if let Some(survey) = &this.survey {
                        survey.abandon();
                    }
                    this.persist_frame(window, cx);
                });
            }
            true
        });
        let mut this = BakeDialog {
            library,
            survey: None,
            candidates: Vec::new(),
            error: None,
            picked: [false; 3],
            now_art,
            backdrop: WindowBackdrop::default(),
            _backdrop_changed,
        };
        this.begin(cx);
        this
    }

    /// The model is read here so the counts match the vectors the library ranks
    /// by; another model's rows describe the same tracks differently.
    fn begin(&mut self, cx: &mut Context<Self>) {
        let db_path = self.library.read(cx).db_path();
        let model = rox_services::acoustic::acoustic_source().id().to_owned();
        let dir = lyrics_dir();
        let survey = Arc::new(job::Survey::default());
        self.survey = Some(survey.clone());
        cx.spawn(async move |this, cx| {
            let found = cx
                .background_executor()
                .spawn({
                    let survey = survey.clone();
                    async move { job::survey(&db_path, &model, Some(&dir), &survey) }
                })
                .await;
            this.update(cx, |this, cx| this.settle(found, cx)).ok();
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let live = this.update(cx, |this, cx| {
                    cx.notify();
                    this.survey.is_some()
                });
                if !matches!(live, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// Every source with something to write starts ticked.
    fn settle(&mut self, found: Result<Vec<Candidate>, String>, cx: &mut Context<Self>) {
        self.survey = None;
        match found {
            Ok(found) => {
                self.candidates = found;
                for (at, source) in Source::ALL.into_iter().enumerate() {
                    self.picked[at] = self.counts(source).writes > 0;
                }
            }
            Err(e) => self.error = Some(e.into()),
        }
        cx.notify();
    }

    fn counts(&self, source: Source) -> Counts {
        bake::counts(&self.candidates, source)
    }

    fn sources(&self) -> Vec<Source> {
        Source::ALL
            .into_iter()
            .enumerate()
            .filter(|(at, source)| self.picked[*at] && self.counts(*source).writes > 0)
            .map(|(_, source)| source)
            .collect()
    }

    fn embed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sources = self.sources();
        let items = bake::merge(&self.candidates, &sources);
        if items.is_empty() {
            return;
        }
        let skipped: usize = sources
            .iter()
            .map(|source| self.counts(*source).skipped)
            .sum();
        self.persist_frame(window, cx);
        job::start(self.library.clone(), items, skipped, cx);
        window.remove_window();
    }

    fn persist_frame(&self, window: &Window, _cx: &App) {
        let frame = window.window_bounds().get_bounds();
        Settings::update(move |s| {
            s.windows.bake_dialog = Some(LayoutSize {
                width: frame.size.width.into(),
                height: frame.size.height.into(),
            });
        });
    }

    /// A source with nothing to write stays visible and inert, so its count reads.
    fn source_row(&self, at: usize, source: Source, cx: &mut Context<Self>) -> Stateful<Div> {
        let counts = self.counts(source);
        let live = counts.writes > 0;
        let on = self.picked[at] && live;
        let detail = match (counts.writes, counts.skipped) {
            (0, 0) => rox_i18n::t!("bake-detail-nothing"),
            (0, skipped) => rox_i18n::t!("bake-detail-only-skipped", skipped = skipped as u64),
            (writes, 0) => rox_i18n::t!("bake-detail-writes", count = writes as u64),
            (writes, skipped) => rox_i18n::t!(
                "bake-detail-writes-skipped",
                count = writes as u64,
                skipped = skipped as u64,
            ),
        };
        div()
            .id(("bake-source", at))
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .when(live, |d| {
                d.cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.picked[at] = !this.picked[at];
                        cx.notify();
                    }))
            })
            .child(settings_ui::checkbox(on))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(if live {
                        palette::text_bright()
                    } else {
                        palette::text_faint()
                    })
                    .child(match source {
                        Source::Lyrics => rox_i18n::t!("bake-source-lyrics"),
                        Source::Gain => rox_i18n::t!("bake-source-gain"),
                        Source::Acoustic => rox_i18n::t!("bake-source-acoustic"),
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(detail),
            )
    }

    fn status(&self) -> Option<(SharedString, gpui::Rgba)> {
        if let Some(e) = &self.error {
            return Some((
                rox_i18n::t!("bake-error-read", error = e.to_string()),
                palette::tone_bad(),
            ));
        }
        if let Some(survey) = &self.survey {
            let total = survey.total();
            let line = if total == 0 {
                rox_i18n::t!("bake-survey-counting")
            } else {
                rox_i18n::t!(
                    "bake-survey-progress",
                    done = survey.done().min(total) as u64,
                    total = total as u64,
                )
            };
            return Some((line, palette::text_muted()));
        }
        if self.sources().is_empty() {
            return Some((rox_i18n::t!("bake-nothing-to-embed"), palette::tone_warn()));
        }
        None
    }

    fn footer(&self, ready: bool, cx: &mut Context<Self>) -> Div {
        let hint = match self.status() {
            Some((line, color)) => div()
                .text_xs()
                .text_color(color)
                .child(line)
                .into_any_element(),
            None => kbd_line([
                Seg::Text(rox_i18n::t!("bake-hint-before")),
                Seg::Key(rox_i18n::t!("bake-hint-key")),
                Seg::Text(rox_i18n::t!("bake-hint-after")),
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
                        rox_i18n::t!("bake-embed"),
                        icons::UPLOAD,
                        !ready,
                        cx.listener(|this, _, window, cx| this.embed(window, cx)),
                    ))
                    .child(settings_ui::small_button(
                        rox_i18n::t!("bake-cancel"),
                        icons::CLOSE,
                        false,
                        cx.listener(|this, _, window, cx| {
                            if let Some(survey) = &this.survey {
                                survey.abandon();
                            }
                            this.persist_frame(window, cx);
                            window.remove_window();
                        }),
                    )),
            )
    }
}

impl Render for BakeDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ready = self.survey.is_none() && !self.sources().is_empty();
        // A file can be in several rows, so the heading counts the merge.
        let total = ready.then(|| {
            let picked = bake::merge(&self.candidates, &self.sources()).len();
            div()
                .text_xs()
                .text_color(palette::text())
                .child(rox_i18n::t!("bake-rewrites", count = picked as u64))
                .into_any_element()
        });
        let rows = Source::ALL
            .into_iter()
            .enumerate()
            .map(|(at, source)| self.source_row(at, source, cx))
            .collect::<Vec<_>>();
        let body = div()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!("bake-intro")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(palette::text_faint())
                    .child(rox_i18n::t!("bake-formats")),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_XS)
                    .pt(tokens::SPACE_XS)
                    .children(rows),
            );

        div()
            .size_full()
            .flex()
            .flex_col()
            .key_context(CONTEXT)
            .on_action(cx.listener(|this, _: &Embed, window, cx| this.embed(window, cx)))
            .bg(palette::bg_elevated())
            .text_color(palette::text_bright())
            .text_sm()
            .children(self.backdrop.layer(&self.now_art, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .bg(palette::bg_elevated())
                    .p(tokens::SPACE_MD)
                    .child(section(rox_i18n::t_static("bake-title"), total, body)),
            )
            .child(self.footer(ready, cx))
    }
}
