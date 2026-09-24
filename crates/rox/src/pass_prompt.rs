//! The start prompt for the long library passes: what the pass will do, how
//! long it should take on this machine, and the worker count that moves the
//! number. No pass starts on a bare button press.
//!
//! Shared by the settings and tasks windows so the two can't drift. A host
//! holds a [`Prompt`] and implements [`Host`]; the probe, the debounced write
//! and the start all happen here.

use std::time::Duration;

use gpui::{Context, Div, Entity, KeyDownEvent, SharedString, Window, div, prelude::*, px};

use crate::{embeddings, replaygain_job, romanize_job, sortnames_job, tempo_job};
use rox_core::settings::{AcousticSave, ReplayGainSave, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel;
use rox_panel_kit::ScrubState;
use rox_panel_kit::ui::{self as settings_ui, dialog_button, dialog_icon_button};
use rox_services::catalog::Library;

/// How long a worker drag settles before the count is written. Writing per tick
/// rewrites the whole settings file per frame, which lags the slider.
const SETTLE: Duration = Duration::from_millis(200);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// Analyze Missing on the acoustic section.
    Acoustic,
    /// Measure Missing on the ReplayGain section.
    ReplayGain,
    /// `retry_refused` is the Retry Refused button: the same job over the
    /// tracks an earlier pass heard no beat in.
    Tempo { retry_refused: bool },
    /// The scope switches inside the dialog, since the two scopes differ by an
    /// hour on the same library.
    SortNames { scope: sortnames_job::Scope },
    /// No options: there's no narrower half worth offering.
    Romanize,
}

pub trait Host: 'static + Sized {
    fn prompt(&self) -> Option<&Prompt>;
    fn prompt_mut(&mut self) -> &mut Option<Prompt>;
    /// The dialog takes the host's shared dialog focus while it's up, since
    /// keys only reach listeners on the focus path.
    fn dialog_focus(&self) -> &gpui::FocusHandle;
    fn value_edit(&self) -> &panel::ValueEdit;
    /// A pass started or a probe measured something; the host re-reads whatever
    /// it caches about the passes.
    fn pass_changed(&mut self, _cx: &mut Context<Self>) {}
    /// A prompt raised by [`raise_for_switch`] was cancelled, so the host turns
    /// the switch back off.
    fn pass_refused(&mut self, _pass: Pass, _cx: &mut Context<Self>) {}
}

pub struct Prompt {
    pass: Pass,
    library: Entity<Library>,
    /// Live here; written to settings when a drag settles and again before the
    /// pass reads it.
    workers: usize,
    scrub: ScrubState,
    missing: u64,
    pace: f32,
    model: String,
    save: ReplayGainSave,
    acoustic_save: AcousticSave,
    probing: bool,
    error: Option<String>,
    /// Both sort-name scopes, non-Latin first, counted once so switching
    /// reprices without another walk.
    sort_scopes: (u64, u64),
    /// Romanization values that need the dictionary.
    kanji: u64,
    /// Cancel then also turns the host's switch back off.
    switched: bool,
    generation: u32,
}

impl Prompt {
    fn estimate(&self) -> Option<String> {
        rox_core::pace::estimate(self.pace, self.missing, self.workers)
    }

    fn persist(&self) {
        let workers = self.workers;
        match self.pass {
            Pass::Acoustic => Settings::update(move |s| s.acoustic_workers = workers),
            Pass::ReplayGain => Settings::update(move |s| s.replaygain_workers = workers),
            Pass::Tempo { .. } => Settings::update(move |s| s.tempo_workers = workers),
            // One worker: MusicBrainz's rate limit is the pace.
            Pass::SortNames { .. } => {}
            Pass::Romanize => {}
        }
    }

    /// The sort-name fill and the romanization run one worker, and a slider
    /// that can't change the estimate would be a lie.
    fn takes_workers(&self) -> bool {
        !matches!(self.pass, Pass::SortNames { .. } | Pass::Romanize)
    }

    /// Only the romanization pass has one: kanji skipped without the
    /// dictionary. A note rather than a refusal, since the rest still runs.
    fn shortfall(&self) -> Option<SharedString> {
        match self.pass {
            Pass::Romanize if self.kanji > 0 && !romanize_job::dictionary_installed() => {
                Some(rox_i18n::t!(
                    "pass-romanize-skips-kanji",
                    kanji = self.kanji,
                    total = self.missing
                ))
            }
            _ => None,
        }
    }
}

pub fn cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

pub fn raise<V: Host>(this: &mut V, pass: Pass, library: Entity<Library>, cx: &mut Context<V>) {
    let settings = Settings::load();
    let source = rox_services::acoustic::acoustic_source();
    let sort_scopes = match pass {
        Pass::SortNames { .. } => library
            .read(cx)
            .projection()
            .map(|projection| {
                let all = sortnames_job::backlog(projection, sortnames_job::Scope::All);
                let non_latin = all
                    .iter()
                    .filter(|name| !sortnames_job::is_latin(name))
                    .count();
                (non_latin as u64, all.len() as u64)
            })
            .unwrap_or_default(),
        _ => (0, 0),
    };
    let romanize = match pass {
        Pass::Romanize => {
            let library = library.read(cx);
            let stale = romanize_job::stale(&library.db_path());
            library
                .projection()
                .map(|projection| romanize_job::backlog(projection, &stale))
                .unwrap_or_default()
        }
        _ => romanize_job::Backlog::default(),
    };
    let (missing, pace, workers) = match pass {
        Pass::Acoustic => (
            library.read(cx).acoustic_coverage(source.id()).missing() as u64,
            settings
                .session
                .acoustic_pace
                .get(source.id())
                .copied()
                .unwrap_or_default(),
            settings.acoustic_workers,
        ),
        Pass::ReplayGain => (
            library.read(cx).replaygain_breakdown().missing,
            settings.session.replaygain_pace,
            settings.replaygain_workers,
        ),
        // The refused pile, not the missing one: those are the tracks the retry
        // decodes.
        Pass::Tempo { retry_refused } => {
            let split = library.read(cx).bpm_breakdown();
            (
                if retry_refused {
                    split.refused
                } else {
                    split.missing
                },
                settings.session.tempo_pace,
                settings.tempo_workers,
            )
        }
        // The rate limit is the pace, so this one never offers a probe.
        Pass::SortNames { scope } => (
            match scope {
                sortnames_job::Scope::NonLatin => sort_scopes.0,
                sortnames_job::Scope::All => sort_scopes.1,
            },
            sortnames_job::PACE,
            1,
        ),
        // A measured pace, not a constant: kana and kanji differ by an order of
        // magnitude.
        Pass::Romanize => (
            romanize.items.len() as u64,
            settings.session.romanize_pace,
            1,
        ),
    };
    let kanji = romanize.kanji();
    *this.prompt_mut() = Some(Prompt {
        pass,
        library,
        workers: workers.clamp(1, cores()),
        scrub: ScrubState::default(),
        missing,
        pace,
        model: source.label(),
        save: settings.replay_gain.save,
        acoustic_save: settings.acoustic_save,
        probing: false,
        error: None,
        sort_scopes,
        kanji,
        switched: false,
        generation: 0,
    });
    cx.notify();
}

/// A switch that was just turned on: cancelling answers the switch too, through
/// [`Host::pass_refused`]. Without this, a switch over an unmeasured library
/// would start hours of decoding nobody priced.
pub fn raise_for_switch<V: Host>(
    this: &mut V,
    pass: Pass,
    library: Entity<Library>,
    cx: &mut Context<V>,
) {
    raise(this, pass, library, cx);
    if let Some(prompt) = this.prompt_mut() {
        prompt.switched = true;
    }
}

fn cancel<V: Host>(this: &mut V, cx: &mut Context<V>) {
    if let Some(prompt) = this.prompt_mut().take() {
        // Cancel still keeps the worker count: it's the setting either way.
        prompt.persist();
        if prompt.switched {
            this.pass_refused(prompt.pass, cx);
        }
        this.pass_changed(cx);
    }
    cx.notify();
}

fn start<V: Host>(this: &mut V, cx: &mut Context<V>) {
    let Some(prompt) = this.prompt_mut().take() else {
        return;
    };
    // The debounced write may still be pending, and the pass reads the file.
    prompt.persist();
    match prompt.pass {
        Pass::Acoustic => embeddings::start(prompt.library.clone(), cx),
        Pass::ReplayGain => replaygain_job::start(prompt.library.clone(), cx),
        Pass::Tempo { retry_refused } => {
            tempo_job::start(prompt.library.clone(), retry_refused, cx)
        }
        Pass::SortNames { scope } => sortnames_job::start(prompt.library.clone(), scope, cx),
        Pass::Romanize => romanize_job::start(prompt.library.clone(), cx),
    }
    this.pass_changed(cx);
    // The pass outlives this window, so hand the user the tasks window.
    crate::tasks_window::open(cx);
    cx.notify();
}

fn probe<V: Host>(this: &mut V, cx: &mut Context<V>) {
    let Some(prompt) = this.prompt_mut() else {
        return;
    };
    if prompt.probing {
        return;
    }
    prompt.probing = true;
    prompt.error = None;
    let pass = prompt.pass;
    let db_path = prompt.library.read(cx).db_path();
    cx.notify();
    let source = matches!(pass, Pass::Acoustic).then(rox_services::acoustic::acoustic_source);
    // Sampled off the projection on the UI thread; the romanization probe reads
    // values, not files.
    let sample = match pass {
        Pass::Romanize => prompt
            .library
            .read(cx)
            .projection()
            .map(|projection| {
                let stale = romanize_job::stale(&db_path);
                let mut items = romanize_job::backlog(projection, &stale).items;
                items.truncate(100);
                items
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    cx.spawn(async move |this, cx| {
        let measured = cx
            .background_executor()
            .spawn(async move {
                match (pass, source) {
                    (Pass::Acoustic, Some(source)) => rox_acoustic::measure_pace(&source, &db_path)
                        .map(|pace| Measured::Acoustic(source.id().to_string(), pace)),
                    // Can't happen: the extractor always resolves.
                    (Pass::Acoustic, None) => Err("no extractor to time".to_string()),
                    (Pass::ReplayGain, _) => {
                        replaygain_job::measure_pace(&db_path).map(Measured::ReplayGain)
                    }
                    // Samples the pile this prompt would run.
                    (Pass::Tempo { retry_refused }, _) => {
                        tempo_job::measure_pace(&db_path, retry_refused).map(Measured::Tempo)
                    }
                    // Can't happen: the sort-name prompt always has an
                    // estimate, so this button isn't drawn.
                    (Pass::SortNames { .. }, _) => Err(
                        "the sort-name pass is paced by MusicBrainz's rate limit, so there's \
                         nothing to time here"
                            .to_string(),
                    ),
                    (Pass::Romanize, _) => {
                        romanize_job::measure_pace(sample).map(Measured::Romanize)
                    }
                }
            })
            .await;
        this.update(cx, |this, cx| {
            let Some(prompt) = this.prompt_mut() else {
                remember(&measured);
                return;
            };
            prompt.probing = false;
            match &measured {
                Ok(measured) => {
                    prompt.pace = measured.pace();
                    // The probe keeps what it built, so the missing count
                    // moved.
                    if matches!(prompt.pass, Pass::Acoustic) {
                        let source = rox_services::acoustic::acoustic_source();
                        prompt.missing = prompt
                            .library
                            .read(cx)
                            .acoustic_coverage(source.id())
                            .missing() as u64;
                    }
                }
                Err(e) => {
                    log::warn!("pace probe: {e}");
                    prompt.error = Some(e.clone());
                }
            }
            remember(&measured);
            this.pass_changed(cx);
            cx.notify();
        })
        .ok();
    })
    .detach();
}

enum Measured {
    Acoustic(String, f32),
    ReplayGain(f32),
    Tempo(f32),
    Romanize(f32),
}

impl Measured {
    fn pace(&self) -> f32 {
        match self {
            Measured::Acoustic(_, pace)
            | Measured::ReplayGain(pace)
            | Measured::Tempo(pace)
            | Measured::Romanize(pace) => *pace,
        }
    }
}

fn remember(measured: &Result<Measured, String>) {
    let Ok(measured) = measured else {
        return;
    };
    let pace = measured.pace();
    match measured {
        Measured::Acoustic(id, _) => {
            let id = id.clone();
            Settings::update(move |s| {
                s.session.acoustic_pace.insert(id.clone(), pace);
            });
        }
        Measured::ReplayGain(_) => Settings::update(move |s| s.session.replaygain_pace = pace),
        Measured::Tempo(_) => Settings::update(move |s| s.session.tempo_pace = pace),
        Measured::Romanize(_) => Settings::update(move |s| s.session.romanize_pace = pace),
    }
}

struct Copy {
    title: SharedString,
    body: SharedString,
    action: SharedString,
}

fn copy(prompt: &Prompt) -> Copy {
    match prompt.pass {
        Pass::Acoustic => {
            let lands = match prompt.acoustic_save {
                AcousticSave::Database => rox_i18n::t!("pass-acoustic-lands-database"),
                AcousticSave::Tags => rox_i18n::t!("pass-acoustic-lands-tags"),
            };
            Copy {
                title: rox_i18n::t!("pass-acoustic-title", count = prompt.missing),
                body: rox_i18n::t!(
                    "pass-acoustic-body",
                    model = prompt.model.clone(),
                    lands = lands.to_string(),
                ),
                action: rox_i18n::t!("pass-analyze"),
            }
        }
        Pass::ReplayGain => {
            let lands = match prompt.save {
                ReplayGainSave::Database => rox_i18n::t!("pass-replaygain-lands-database"),
                ReplayGainSave::Tags => rox_i18n::t!("pass-replaygain-lands-tags"),
            };
            Copy {
                title: rox_i18n::t!("pass-replaygain-title", count = prompt.missing),
                body: rox_i18n::t!("pass-replaygain-body", lands = lands.to_string()),
                action: rox_i18n::t!("pass-measure"),
            }
        }
        Pass::Tempo {
            retry_refused: true,
        } => Copy {
            title: rox_i18n::t!("pass-tempo-retry-title", count = prompt.missing),
            body: rox_i18n::t!("pass-tempo-retry-body"),
            action: rox_i18n::t!("pass-analyze"),
        },
        Pass::Tempo { .. } => Copy {
            title: rox_i18n::t!("pass-tempo-title", count = prompt.missing),
            body: rox_i18n::t!("pass-tempo-body"),
            action: rox_i18n::t!("pass-analyze"),
        },
        Pass::SortNames { .. } => Copy {
            title: rox_i18n::t!("pass-sortnames-title", count = prompt.missing),
            body: rox_i18n::t!("pass-sortnames-body"),
            action: rox_i18n::t!("pass-fill"),
        },
        Pass::Romanize => Copy {
            title: rox_i18n::t!("pass-romanize-title", count = prompt.missing),
            body: rox_i18n::t!("pass-romanize-body"),
            action: rox_i18n::t!("pass-romanize"),
        },
    }
}

/// The prompt, drawn by the host over its whole window body. No click-away: the
/// buttons, Enter and Escape are the ways out.
pub fn overlay<V: Host>(this: &V, window: &mut Window, cx: &mut Context<V>) -> Option<Div> {
    let prompt = this.prompt()?;
    let cores = cores();
    let copy = copy(prompt);
    let estimate = prompt.estimate();
    let shortfall = prompt.shortfall();
    let timing = match (&estimate, prompt.probing, &prompt.error) {
        (_, true, _) => rox_i18n::t!("pass-timing"),
        (Some(estimate), _, _) => rox_i18n::t!(
            "pass-estimate-at",
            estimate = estimate.clone(),
            workers_phrase = rox_core::pace::workers_phrase(prompt.workers),
        ),
        (None, _, Some(error)) => rox_i18n::t!("pass-timing-failed", error = error.clone()),
        (None, _, None) => rox_i18n::t!("pass-no-estimate"),
    };
    let failed = estimate.is_none() && !prompt.probing && prompt.error.is_some();
    // Only offered with nothing measured: a second opinion off three tracks is
    // worse than a real pass's pace.
    let probing = prompt.probing;
    let probe_button = estimate.is_none().then(|| {
        dialog_icon_button(
            if probing {
                rox_i18n::t!("pass-estimating")
            } else {
                rox_i18n::t!("pass-estimate-button")
            },
            icons::GAUGE,
            probing,
            cx.listener(|this: &mut V, _, _, cx| probe(this, cx)),
        )
    });
    if !this.dialog_focus().contains_focused(window, cx) {
        window.focus(this.dialog_focus());
    }
    Some(
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .track_focus(this.dialog_focus())
            .on_key_down(
                cx.listener(|this: &mut V, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.modifiers.modified() {
                        return;
                    }
                    match event.keystroke.key.as_str() {
                        "escape" => cancel(this, cx),
                        "enter" if this.dialog_focus().is_focused(window) => start(this, cx),
                        _ => return,
                    }
                    cx.stop_propagation();
                }),
            )
            .p(tokens::SPACE_MD)
            .bg(gpui::rgba(0x00000066))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(400.))
                    .max_w_full()
                    .rounded(tokens::RADIUS)
                    .bg(palette::bg_menu_opaque())
                    .border_1()
                    .border_color(palette::border_light())
                    .shadow_md()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_MD)
                            .p(tokens::SPACE_MD)
                            .child(div().child(copy.title))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(palette::text_muted())
                                    .child(copy.body),
                            )
                            .children(prompt.takes_workers().then(|| {
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .gap(tokens::SPACE_MD)
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_xs()
                                            .child(rox_i18n::t!("pass-workers")),
                                    )
                                    .child(div().flex_1().child(settings_ui::scalar_sized(
                                        &prompt.scrub,
                                        this.value_edit(),
                                        prompt.workers.min(cores) as f32,
                                        settings_ui::span(1.0, cores as f32, "").hard(),
                                        panel::SliderWidth::Fill,
                                        set_workers::<V>,
                                        cx,
                                    )))
                            }))
                            .children(scope_row(prompt, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(tokens::SPACE_SM)
                            .px(tokens::SPACE_MD)
                            .py(tokens::SPACE_SM)
                            .border_t_1()
                            .border_color(palette::border())
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if failed {
                                        palette::tone_warn()
                                    } else {
                                        palette::text_muted()
                                    })
                                    .child(timing),
                            )
                            .children(shortfall.map(|note| {
                                div().text_xs().text_color(palette::tone_warn()).child(note)
                            }))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .items_center()
                                    .justify_between()
                                    .gap(tokens::SPACE_SM)
                                    .child(div().flex_none().children(probe_button))
                                    .child(
                                        div()
                                            .flex()
                                            .flex_row()
                                            .flex_none()
                                            .items_center()
                                            .gap(tokens::SPACE_SM)
                                            .child(dialog_button(
                                                rox_i18n::t!("settings-common-cancel"),
                                                false,
                                                cx.listener(|this: &mut V, _, _, cx| {
                                                    cancel(this, cx)
                                                }),
                                            ))
                                            .child(dialog_button(
                                                copy.action,
                                                true,
                                                cx.listener(|this: &mut V, _, _, cx| {
                                                    start(this, cx)
                                                }),
                                            )),
                                    ),
                            ),
                    ),
            ),
    )
}

fn scope_row<V: Host>(prompt: &Prompt, cx: &mut Context<V>) -> Option<gpui::Stateful<Div>> {
    let Pass::SortNames { scope } = prompt.pass else {
        return None;
    };
    let on = scope == sortnames_job::Scope::All;
    Some(
        div()
            .id("pass-scope")
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .cursor_pointer()
            .on_click(cx.listener(|this: &mut V, _, _, cx| {
                let Some(prompt) = this.prompt_mut() else {
                    return;
                };
                let Pass::SortNames { scope } = &mut prompt.pass else {
                    return;
                };
                *scope = match scope {
                    sortnames_job::Scope::NonLatin => sortnames_job::Scope::All,
                    sortnames_job::Scope::All => sortnames_job::Scope::NonLatin,
                };
                prompt.missing = match scope {
                    sortnames_job::Scope::NonLatin => prompt.sort_scopes.0,
                    sortnames_job::Scope::All => prompt.sort_scopes.1,
                };
                cx.notify();
            }))
            .child(settings_ui::checkbox(on))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .child(rox_i18n::t!("pass-sortnames-scope-all")),
            ),
    )
}

fn set_workers<V: Host>(this: &mut V, value: f32, cx: &mut Context<V>) {
    let Some(prompt) = this.prompt_mut() else {
        return;
    };
    prompt.workers = (value.round() as usize).clamp(1, cores());
    prompt.generation += 1;
    let generation = prompt.generation;
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(SETTLE).await;
        this.update(cx, |this, _| {
            // Re-read at fire time so the last tick of a burst writes what the
            // slider ended on.
            if let Some(prompt) = this.prompt()
                && prompt.generation == generation
            {
                prompt.persist();
            }
        })
        .ok();
    })
    .detach();
    cx.notify();
}
