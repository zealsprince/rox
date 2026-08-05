//! The start prompt for the long library passes: what the pass is about to
//! do, how long that should take on this machine, and the worker count that
//! moves the number.
//!
//! Neither pass starts on a bare button press. Both cost an afternoon on a
//! large library, both scale with workers, and one of them can rewrite every
//! audio file in it. The prompt is where that trade gets made: the estimate
//! is priced against the slider live, so a usable machine against a shorter
//! wait is visible while the choice is happening rather than described in a
//! settings row nobody reads first.
//!
//! It lives here rather than in the settings window because it is no longer
//! that window's dialog. The tasks window starts the same passes, and a
//! second copy would be two dialogs drifting apart: one with the slider and
//! the estimate, the other with a button that just goes. A host wires itself
//! in by holding a [`Prompt`] and implementing [`Host`]; the probe, the
//! debounced write, and the start all happen in here.

use std::time::Duration;

use gpui::{div, prelude::*, px, Context, Div, Entity, SharedString};

use crate::assets::icons;
use crate::catalog::Library;
use crate::design::{palette, tokens};
use crate::panel::{self, ScrubState};
use crate::settings::ui::{self as settings_ui, dialog_button, small_button};
use crate::settings::{ReplayGainSave, Settings};
use crate::{embeddings, replaygain_job};

/// How long a worker drag settles before the count is written. A scrub
/// applies per frame, and writing through on every tick means reading,
/// parsing, and rewriting the whole settings file per frame, which is felt
/// as the slider lagging the pointer. The prompt holds the live value either
/// way, so only the file write waits.
const SETTLE: Duration = Duration::from_millis(200);

/// Which pass the prompt is offering.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// The acoustic analysis pass, Analyze Missing.
    Acoustic,
    /// The ReplayGain measurement pass, Measure Missing.
    ReplayGain,
}

/// A window that can raise the prompt. It holds the state and hands this
/// module a way at it; the module owns everything the dialog does.
pub trait Host: 'static + Sized {
    fn prompt(&self) -> Option<&Prompt>;
    fn prompt_mut(&mut self) -> &mut Option<Prompt>;
    /// The host's click-to-type slider state, shared with its other sliders
    /// so only one value is ever being typed into.
    fn value_edit(&self) -> &panel::ValueEdit;
    /// A pass started, or a probe measured something. Whatever the host
    /// caches about the passes - counts, paces, the running job - has moved,
    /// and this is where it re-reads them.
    fn pass_changed(&mut self, _cx: &mut Context<Self>) {}
}

/// A raised prompt: which pass, over which library, at what count, and
/// whatever the estimate has to say so far.
pub struct Prompt {
    pass: Pass,
    /// The catalog the pass will run over, held for the prompt's life so the
    /// dialog never has to ask its host for one mid-step.
    library: Entity<Library>,
    /// The count the slider drives. Live here; written to settings when a
    /// drag settles and again before the pass reads it.
    workers: usize,
    scrub: ScrubState,
    /// Tracks the pass would work through, counted when this was raised.
    missing: u64,
    /// Worker-seconds a track cost the last time a pass ran here, 0 for
    /// never. What the estimate is priced off.
    pace: f32,
    /// The acoustic model's name, for a line that has to say whose count
    /// it's quoting.
    model: String,
    /// Where a measured gain lands, worth saying out loud because one of the
    /// two answers rewrites the audio files.
    save: ReplayGainSave,
    probing: bool,
    error: Option<String>,
    /// Bumped per slider tick so only the last one writes.
    generation: u32,
}

impl Prompt {
    /// What the rest of the pass should take at the current count, or None
    /// with nothing measured on this machine yet.
    fn estimate(&self) -> Option<String> {
        crate::pace::estimate(self.pace, self.missing, self.workers)
    }

    /// Write the worker count where the pass will read it.
    fn persist(&self) {
        let workers = self.workers;
        match self.pass {
            Pass::Acoustic => Settings::update(move |s| s.acoustic_workers = workers),
            Pass::ReplayGain => Settings::update(move |s| s.replaygain_workers = workers),
        }
    }
}

/// Every worker the machine has. The prompt is a deliberate choice made in
/// front of an estimate, so the ceiling is the machine's rather than one a
/// window picked on the user's behalf.
pub fn cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Raise the prompt for a pass: count what it would get through, and read
/// back what the last one cost here. Clears whatever the last probe had to
/// say, so a failure from one visit doesn't greet the next.
pub fn raise<V: Host>(this: &mut V, pass: Pass, library: Entity<Library>, cx: &mut Context<V>) {
    let settings = Settings::load();
    let source = crate::settings::acoustic_source();
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
    };
    *this.prompt_mut() = Some(Prompt {
        pass,
        library,
        workers: workers.clamp(1, cores()),
        scrub: ScrubState::default(),
        missing,
        pace,
        model: source.label(),
        save: settings.replay_gain.save,
        probing: false,
        error: None,
        generation: 0,
    });
    cx.notify();
}

/// Take the prompt down without starting anything.
fn cancel<V: Host>(this: &mut V, cx: &mut Context<V>) {
    if let Some(prompt) = this.prompt_mut().take() {
        // The slider may have moved since the last settle, and the count is
        // the setting either way: a prompt is a place to set workers as much
        // as a place to start a pass.
        prompt.persist();
        this.pass_changed(cx);
    }
    cx.notify();
}

/// Start the pass the prompt was offering and take the prompt down.
fn start<V: Host>(this: &mut V, cx: &mut Context<V>) {
    let Some(prompt) = this.prompt_mut().take() else {
        return;
    };
    // The debounced write may still be pending and the pass reads the file:
    // a drag followed straight away by a click would otherwise run the count
    // the slider was at before the drag.
    prompt.persist();
    match prompt.pass {
        Pass::Acoustic => embeddings::start(prompt.library.clone(), cx),
        Pass::ReplayGain => replaygain_job::start(prompt.library.clone(), cx),
    }
    this.pass_changed(cx);
    // The pass outlives whichever window started it, so hand the user
    // something that does too: the tasks window carries the count, the
    // estimate, and the stop button.
    crate::tasks_window::open(cx);
    cx.notify();
}

/// Time a few tracks so the prompt can price the rest, the Estimate button.
/// Runs on the background executor because it decodes real files; the prompt
/// stays up and says it's working.
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
    // Resolved here, on the UI thread, so the probe measures the model the
    // prompt is talking about.
    let source = matches!(pass, Pass::Acoustic).then(crate::settings::acoustic_source);
    cx.spawn(async move |this, cx| {
        let measured = cx
            .background_executor()
            .spawn(async move {
                match &source {
                    Some(source) => embeddings::measure_pace(source, &db_path)
                        .map(|pace| (Some(source.id().to_string()), pace)),
                    None => replaygain_job::measure_pace(&db_path).map(|pace| (None, pace)),
                }
            })
            .await;
        this.update(cx, |this, cx| {
            let Some(prompt) = this.prompt_mut() else {
                // The dialog closed while the probe ran. What it measured is
                // still worth keeping, so it goes to settings below either
                // way; only the dialog's own copy is gone.
                remember(&measured);
                return;
            };
            prompt.probing = false;
            match &measured {
                Ok((_, pace)) => {
                    prompt.pace = *pace;
                    // The probe kept whatever it built, so the count the
                    // estimate multiplies has moved.
                    if matches!(prompt.pass, Pass::Acoustic) {
                        let source = crate::settings::acoustic_source();
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

/// Keep what a probe measured, so the next prompt on this machine opens with
/// a number instead of an offer to go and find one.
fn remember(measured: &Result<(Option<String>, f32), String>) {
    let Ok((model, pace)) = measured else {
        return;
    };
    let (model, pace) = (model.clone(), *pace);
    Settings::update(move |s| match model {
        Some(id) => {
            s.session.acoustic_pace.insert(id, pace);
        }
        None => s.session.replaygain_pace = pace,
    });
}

/// Everything about the dialog that depends on which pass it's offering,
/// resolved once so the dialog itself is built one way.
struct Copy {
    title: String,
    body: String,
    action: &'static str,
}

fn copy(prompt: &Prompt) -> Copy {
    match prompt.pass {
        Pass::Acoustic => Copy {
            title: format!("Analyze {} tracks?", prompt.missing),
            body: format!(
                "{} works out what each one sounds like, so the library can find \
                 music that resembles what's playing. Everything runs on this \
                 machine, and what's described already is left alone.",
                prompt.model
            ),
            action: "Analyze",
        },
        Pass::ReplayGain => {
            // Where the numbers land is worth saying here: tags mode rewrites
            // the audio files, which is not something to learn about
            // afterwards.
            let lands = match prompt.save {
                ReplayGainSave::Database => {
                    "The numbers go in the library database and your files are left alone."
                }
                ReplayGainSave::Tags => {
                    "The numbers are written back into each file's tags, where every \
                     other player reads them."
                }
            };
            Copy {
                title: format!("Measure {} tracks?", prompt.missing),
                body: format!(
                    "Each file is decoded and metered so it can play at the loudness \
                     it was mastered to. Albums are measured whole where every one of \
                     their tracks is missing a gain. {lands}"
                ),
                action: "Measure",
            }
        }
    }
}

/// The prompt itself, or nothing while none is raised. The host drops this
/// at the root of its window body, over everything.
///
/// Same scrim and layering as the settings window's overwrite confirm, and
/// no click-away for the same reason: the buttons are the way out.
pub fn overlay<V: Host>(this: &V, cx: &mut Context<V>) -> Option<Div> {
    let prompt = this.prompt()?;
    let cores = cores();
    let copy = copy(prompt);
    let estimate = prompt.estimate();
    // The estimate is the reason this dialog exists, so it says something
    // either way: the number, why there isn't one yet, or what went wrong
    // reaching for it.
    let timing = match (&estimate, prompt.probing, &prompt.error) {
        (_, true, _) => "Timing a few tracks...".to_string(),
        (Some(estimate), _, _) => format!(
            "{estimate} at {}.",
            crate::pace::workers_phrase(prompt.workers)
        ),
        (None, _, Some(error)) => format!("Couldn't time this library: {error}"),
        (None, _, None) => "Nothing has run on this machine yet, so there's no estimate. \
                            Estimate times a few tracks and works the rest out from there."
            .to_string(),
    };
    // Only offered while there's nothing measured: once there's a real
    // number, the pass itself keeps it honest and a second opinion off three
    // tracks would be the worse of the two.
    let probing = prompt.probing;
    let probe_button = estimate.is_none().then(|| {
        small_button(
            if probing { "Estimating..." } else { "Estimate" },
            icons::GAUGE,
            probing,
            cx.listener(|this: &mut V, _, _, cx| probe(this, cx)),
        )
    });
    Some(
        div()
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000066))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(tokens::SPACE_MD)
                    .w(px(400.))
                    .p(tokens::SPACE_MD)
                    .rounded(tokens::RADIUS)
                    .bg(palette::bg_menu_opaque())
                    .border_1()
                    .border_color(palette::border_light())
                    .shadow_md()
                    .child(div().child(SharedString::from(copy.title)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(palette::text_muted())
                            .child(SharedString::from(copy.body)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(tokens::SPACE_MD)
                            .child(div().flex_none().text_xs().child("Workers"))
                            .child(div().flex_1().child(settings_ui::scalar_sized(
                                &prompt.scrub,
                                this.value_edit(),
                                prompt.workers.min(cores) as f32,
                                settings_ui::span(1.0, cores as f32, "").hard(),
                                panel::SliderWidth::Fill,
                                set_workers::<V>,
                                cx,
                            ))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(tokens::SPACE_MD)
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .text_color(palette::text_muted())
                                    .child(SharedString::from(timing)),
                            )
                            .children(probe_button),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(tokens::SPACE_SM)
                            .child(dialog_button(
                                "Cancel",
                                false,
                                cx.listener(|this: &mut V, _, _, cx| cancel(this, cx)),
                            ))
                            .child(dialog_button(
                                copy.action,
                                true,
                                cx.listener(|this: &mut V, _, _, cx| start(this, cx)),
                            )),
                    ),
            ),
    )
}

/// The worker slider's landing: the live count moves now, the file catches
/// up once the drag settles.
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
            // Re-read at fire time rather than trusting a capture, so the
            // last tick of a burst writes what the slider actually landed on.
            if let Some(prompt) = this.prompt() {
                if prompt.generation == generation {
                    prompt.persist();
                }
            }
        })
        .ok();
    })
    .detach();
    cx.notify();
}
