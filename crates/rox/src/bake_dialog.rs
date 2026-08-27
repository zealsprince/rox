//! The embed dialog: which of the three stored sources to write into the
//! files, and what that would come to.
//!
//! The three save settings each apply to the next write and nothing else, so
//! a library described under Database and then switched to Tags has none of
//! it in the files. This is the catch-up, and it exists as a dialog rather
//! than a button because the honest answer to "what would this do" is three
//! different numbers.
//!
//! Every count here is real: the survey behind it reads the tags of every
//! candidate before the checkboxes say anything, which is why the window
//! opens counting rather than opening ready. A source with nothing to write
//! is disabled with its number showing instead of hidden, since "no lyrics to
//! embed" and "lyrics aren't offered here" are different answers.
//!
//! The run itself belongs to [`crate::bake`], which is app-global: the dialog
//! closes on the press and the tasks window shows the progress and the Stop,
//! the same as a conversion.

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    actions, div, prelude::*, px, size, App, Bounds, Context, Div, Entity, Global, KeyBinding,
    SharedString, Stateful, Subscription, Window, WindowHandle,
};
use gpui_component::Root;

use rox_core::settings::{lyrics_dir, LayoutSize, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_library::bake::{self, Candidate, Counts, Source};
use rox_panel_kit::ui::{self as settings_ui, kbd_line, section, Seg};
use rox_services::backdrop::{NowPlayingArt, WindowBackdrop};
use rox_services::catalog::Library;

/// The run and the survey behind this window. Aliased because
/// [`rox_library::bake`] defines what a bake is and this drives one, and the
/// two names would otherwise be the same word twice in every line.
use crate::bake as job;

/// The dialog's floor. Wide enough that a source's line doesn't wrap, tall
/// enough for the three rows and the note above them.
const MIN: gpui::Size<gpui::Pixels> = gpui::Size {
    width: px(420.),
    height: px(280.),
};

/// How often the window repaints while the survey runs. The survey is a file
/// open per candidate and the readout is a count, so four times a second is
/// more than enough to read by.
const TICK: Duration = Duration::from_millis(250);

actions!(bake_dialog, [Embed]);

/// The key context the window's own bindings scope to.
const CONTEXT: &str = "BakeDialog";

/// The dialog's embed binding; call once at startup. It's bound on the
/// window root, so Enter embeds wherever focus is (a checkbox row, the
/// window itself) rather than only where a button happens to be.
pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("enter", Embed, Some(CONTEXT))]);
}

/// The open dialog, if any. One at a time: it works on the whole library, so
/// a second would be the same window.
struct OpenBake(WindowHandle<Root>);

impl Global for OpenBake {}

/// Open the embed dialog, or bring the open one to the front.
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
    /// The survey while it runs, so the readout has a count and closing the
    /// window can call it off.
    survey: Option<Arc<job::Survey>>,
    /// What the survey found, empty until it finishes.
    candidates: Vec<Candidate>,
    /// Why there's nothing to show, when the survey couldn't run at all.
    error: Option<SharedString>,
    /// One tick per source, in [`Source::ALL`] order.
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
                    // Nobody is waiting for the result any more, and the
                    // survey is a file open per candidate.
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

    /// Set the survey going and keep the window repainting while it runs.
    ///
    /// The model is read here rather than in the survey so the vectors the
    /// dialog counts are the ones the library is actually ranking by: another
    /// model's rows are a different description of the same tracks, and
    /// offering them under one count would be two answers in one number.
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
        // No entity behind a background survey, so nothing would repaint the
        // count on its own.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(TICK).await;
            let live = this.update(cx, |this, cx| {
                cx.notify();
                this.survey.is_some()
            });
            if !matches!(live, Ok(true)) {
                break;
            }
        })
        .detach();
    }

    /// Take the survey's result and tick every source that has something to
    /// write. Ticking them is the right default: someone who opened this
    /// wants what rox is holding to be in the files, and the counts beside
    /// each row are there to untick one by.
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

    /// The sources currently ticked, and that have anything to write.
    fn sources(&self) -> Vec<Source> {
        Source::ALL
            .into_iter()
            .enumerate()
            .filter(|(at, source)| self.picked[*at] && self.counts(*source).writes > 0)
            .map(|(_, source)| source)
            .collect()
    }

    /// Hand the picked sources to the job and get out of the way.
    fn embed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sources = self.sources();
        let items = bake::merge(&self.candidates, &sources);
        if items.is_empty() {
            return;
        }
        // What the dialog counted as skipped for exactly these sources, so
        // the finished line accounts for every file the checkboxes did.
        let skipped: usize = sources
            .iter()
            .map(|source| self.counts(*source).skipped)
            .sum();
        self.persist_frame(window, cx);
        job::start(self.library.clone(), items, skipped, cx);
        window.remove_window();
    }

    /// Write the window frame into the settings file, the restore for the
    /// next open.
    fn persist_frame(&self, window: &Window, _cx: &App) {
        let frame = window.window_bounds().get_bounds();
        Settings::update(move |s| {
            s.windows.bake_dialog = Some(LayoutSize {
                width: frame.size.width.into(),
                height: frame.size.height.into(),
            });
        });
    }

    /// One source's row: its tick, its name, and what it would come to. A
    /// source with nothing to write is inert rather than gone, so the number
    /// that makes it inert is readable.
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

    /// Why the embed won't run yet, when it won't: the read that failed, how
    /// far the survey has got, or that there's nothing left to write. None
    /// once the press would do something, which is when the footer shows the
    /// shortcut instead.
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

    /// The window's own actions, and the shortcut for them or the reason
    /// there isn't one.
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
        // The rows count per source and a file can be in more than one of
        // them, so the heading's number is the merge rather than the sum.
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
                    // The page's own surface over the root's, the same second
                    // pass the settings page takes: the backdrop reads through
                    // only as the surfaces thin.
                    .bg(palette::bg_elevated())
                    .p(tokens::SPACE_MD)
                    .child(section(rox_i18n::t_static("bake-title"), total, body)),
            )
            .child(self.footer(ready, cx))
    }
}
