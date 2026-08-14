//! The start prompt for the long library passes: what the pass is about to
//! do, how long that should take on this machine, and the worker count that
//! moves the number.
//!
//! No pass starts on a bare button press. They all cost an afternoon on a
//! large library, they all scale with workers, and one of them can rewrite
//! every audio file in it. The prompt is where that trade gets made: the estimate
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

use crate::{embeddings, replaygain_job, tempo_job};
use rox_core::settings::{AcousticSave, ReplayGainSave, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel;
use rox_panel_kit::ui::{self as settings_ui, dialog_button, dialog_icon_button};
use rox_panel_kit::ScrubState;
use rox_services::catalog::Library;

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
    /// The tempo pass, Analyze Missing on the Library page.
    Tempo,
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
    /// A prompt raised by [`raise_for_switch`] was cancelled, so the switch
    /// behind it was a no. The host puts it back. Hosts that only ever raise
    /// the prompt from a button never see this.
    fn pass_refused(&mut self, _pass: Pass, _cx: &mut Context<Self>) {}
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
    /// The same for an acoustic vector, and worth saying for the same reason.
    acoustic_save: AcousticSave,
    probing: bool,
    error: Option<String>,
    /// Whether a switch the host just flipped is standing behind this. Cancel
    /// then means the switch was a no as well, and the host hears about it.
    switched: bool,
    /// Bumped per slider tick so only the last one writes.
    generation: u32,
}

impl Prompt {
    /// What the rest of the pass should take at the current count, or None
    /// with nothing measured on this machine yet.
    fn estimate(&self) -> Option<String> {
        rox_core::pace::estimate(self.pace, self.missing, self.workers)
    }

    /// Write the worker count where the pass will read it.
    fn persist(&self) {
        let workers = self.workers;
        match self.pass {
            Pass::Acoustic => Settings::update(move |s| s.acoustic_workers = workers),
            Pass::ReplayGain => Settings::update(move |s| s.replaygain_workers = workers),
            Pass::Tempo => Settings::update(move |s| s.tempo_workers = workers),
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
    let source = rox_services::acoustic::acoustic_source();
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
        Pass::Tempo => (
            library.read(cx).bpm_breakdown().missing,
            settings.session.tempo_pace,
            settings.tempo_workers,
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
        acoustic_save: settings.acoustic_save,
        probing: false,
        error: None,
        switched: false,
        generation: 0,
    });
    cx.notify();
}

/// Raise the prompt for a switch that was just turned on, where the backlog
/// it inherits is the thing being asked about. The same dialog, except that
/// cancelling answers the switch too: the host is told through
/// [`Host::pass_refused`] and puts it back off.
///
/// A switch is a standing instruction and the library it's turned on over is
/// usually unmeasured, so without this the first thing it would do is start a
/// library's worth of decoding nobody priced.
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

/// Take the prompt down without starting anything.
fn cancel<V: Host>(this: &mut V, cx: &mut Context<V>) {
    if let Some(prompt) = this.prompt_mut().take() {
        // The slider may have moved since the last settle, and the count is
        // the setting either way: a prompt is a place to set workers as much
        // as a place to start a pass.
        prompt.persist();
        if prompt.switched {
            this.pass_refused(prompt.pass, cx);
        }
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
        Pass::Tempo => tempo_job::start(prompt.library.clone(), cx),
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
    let source = matches!(pass, Pass::Acoustic).then(rox_services::acoustic::acoustic_source);
    cx.spawn(async move |this, cx| {
        let measured = cx
            .background_executor()
            .spawn(async move {
                match (pass, source) {
                    (Pass::Acoustic, Some(source)) => rox_acoustic::measure_pace(&source, &db_path)
                        .map(|pace| Measured::Acoustic(source.id().to_string(), pace)),
                    // The extractor always resolves, so this is the case
                    // that can't happen rather than one worth a message.
                    (Pass::Acoustic, None) => Err("no extractor to time".to_string()),
                    (Pass::ReplayGain, _) => {
                        replaygain_job::measure_pace(&db_path).map(Measured::ReplayGain)
                    }
                    (Pass::Tempo, _) => tempo_job::measure_pace(&db_path).map(Measured::Tempo),
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
                Ok(measured) => {
                    prompt.pace = measured.pace();
                    // The probe kept whatever it built, so the count the
                    // estimate multiplies has moved.
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

/// What a probe measured, and where it belongs. The acoustic pace is kept
/// per model because the built-in sketch and a network differ by most of an
/// order of magnitude; the other two passes have no model behind them, so
/// each is one number.
enum Measured {
    Acoustic(String, f32),
    ReplayGain(f32),
    Tempo(f32),
}

impl Measured {
    /// Worker-seconds per track, whichever pass it came from.
    fn pace(&self) -> f32 {
        match self {
            Measured::Acoustic(_, pace) | Measured::ReplayGain(pace) | Measured::Tempo(pace) => {
                *pace
            }
        }
    }
}

/// Keep what a probe measured, so the next prompt on this machine opens with
/// a number instead of an offer to go and find one.
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
    }
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
        Pass::Acoustic => {
            // Tags mode rewrites the audio files, which is not something to
            // learn about afterwards, and it can't reach every format, which
            // is not something to work out from a coverage number.
            let lands = match prompt.acoustic_save {
                AcousticSave::Database => {
                    "The results go in the library database and your files are left alone."
                }
                AcousticSave::Tags => {
                    "The results go in the library database and, for MP3 and FLAC, into each \
                     file's own tags as well, so they survive the database being rebuilt. \
                     Other formats keep the database copy only."
                }
            };
            Copy {
                title: format!("Analyze {} tracks?", prompt.missing),
                body: format!(
                    "{} works out what each one sounds like, so the library can find \
                     music that resembles what's playing. Everything runs on this \
                     machine, and what's described already is left alone. {lands}",
                    prompt.model
                ),
                action: "Analyze",
            }
        }
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
        Pass::Tempo => Copy {
            title: format!("Find the tempo of {} tracks?", prompt.missing),
            body: "Two half-minute windows of each file are decoded and the beats counted, \
                   so the library can say what a track runs at. It works best on music \
                   recorded to a click and skips anything it can't call. The numbers go in \
                   the library database and your files are left alone."
                .to_string(),
            action: "Analyze",
        },
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
            rox_core::pace::workers_phrase(prompt.workers)
        ),
        (None, _, Some(error)) => format!("Couldn't time this library: {error}"),
        (None, _, None) => "Nothing has run on this machine yet, so there's no estimate. \
                            Estimate times a few tracks and works the rest out from there."
            .to_string(),
    };
    // A probe that came back with nothing is the one case the line carries
    // bad news, so it reads as a warning rather than as the estimate it
    // stands in for.
    let failed = estimate.is_none() && !prompt.probing && prompt.error.is_some();
    // Only offered while there's nothing measured: once there's a real
    // number, the pass itself keeps it honest and a second opinion off three
    // tracks would be the worse of the two.
    let probing = prompt.probing;
    let probe_button = estimate.is_none().then(|| {
        dialog_icon_button(
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
            // Inset so the card keeps a margin in a window barely wider
            // than it, instead of running edge to edge.
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
                            ),
                    )
                    // The windows' footer, run inside a card: what the pass
                    // costs across the top, what to do about it in a button
                    // row beneath. The cost line ran beside the buttons once,
                    // and with no estimate yet it wraps to a paragraph that
                    // left them no room.
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
                                    .child(SharedString::from(timing)),
                            )
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
                                                "Cancel",
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
