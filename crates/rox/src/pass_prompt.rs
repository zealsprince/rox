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
//! It's defined here rather than in the settings window because it's no
//! longer that window's dialog. The tasks window starts the same passes, and a
//! second copy would be two dialogs drifting apart: one with the slider and
//! the estimate, the other with a button that just goes. A host wires itself
//! in by holding a [`Prompt`] and implementing [`Host`]; the probe, the
//! debounced write, and the start all happen in here.

use std::time::Duration;

use gpui::{div, prelude::*, px, Context, Div, Entity, KeyDownEvent, SharedString, Window};

use crate::{embeddings, replaygain_job, romanize_job, sortnames_job, tempo_job};
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
    /// The tempo pass, Analyze Missing on the Library page. `retry_refused`
    /// is the Retry Refused button rather than a second pass: same worker
    /// count, same estimate, same job, but over the tracks an earlier pass
    /// listened to and heard no beat in instead of the ones nothing has
    /// reached yet. It rides on the variant because everything downstream
    /// (the count the estimate multiplies, the copy, the start call) has to
    /// know which of the two the user is looking at.
    Tempo { retry_refused: bool },
    /// The sort-name fill, Fill Missing on the health window's sort tile.
    /// The scope rides on the variant the way the tempo retry does, since
    /// the count the estimate multiplies and the work list both follow it;
    /// unlike the retry, this one is switched inside the dialog, because
    /// the two scopes are ten minutes and an hour and a half over the same
    /// library and that's a choice to make in front of the estimate.
    SortNames { scope: sortnames_job::Scope },
    /// The romanization pass, Romanize Library. No options: it reaches
    /// everything with a non-Latin value and no sort name, and there's no
    /// narrower half of that worth offering.
    Romanize,
}

/// A window that can raise the prompt. It holds the state and hands this
/// module a way at it; the module owns everything the dialog does.
pub trait Host: 'static + Sized {
    fn prompt(&self) -> Option<&Prompt>;
    fn prompt_mut(&mut self) -> &mut Option<Prompt>;
    /// Where the keyboard sits while the prompt is up. A key event only
    /// reaches listeners along the path to whatever holds focus, so the
    /// dialog takes it for as long as it's asking; the host lends it the
    /// handle it shares with its own dialogs.
    fn dialog_focus(&self) -> &gpui::FocusHandle;
    /// The host's click-to-type slider state, shared with its other sliders
    /// so only one value is ever being typed into.
    fn value_edit(&self) -> &panel::ValueEdit;
    /// A pass started, or a probe measured something. Whatever the host
    /// caches about the passes (counts, paces, the running job) has moved,
    /// and this is where it re-reads them.
    fn pass_changed(&mut self, _cx: &mut Context<Self>) {}
    /// A prompt raised by [`raise_for_switch`] was cancelled, so the switch
    /// behind it was a no. The host puts it back. Hosts that only ever raise
    /// the prompt from a button never see this.
    fn pass_refused(&mut self, _pass: Pass, _cx: &mut Context<Self>) {}
}

/// A raised prompt: which pass, over which library, at what count, and
/// whatever the estimate shows so far.
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
    /// Where a measured gain is written, worth saying out loud because one
    /// of the two options rewrites the audio files.
    save: ReplayGainSave,
    /// The same for an acoustic vector, and worth saying for the same reason.
    acoustic_save: AcousticSave,
    probing: bool,
    error: Option<String>,
    /// The sort-name pass's two scopes as counted when this was raised,
    /// non-Latin first. Held so switching between them reprices without
    /// walking the symbol tables again; (0, 0) for every other pass.
    sort_scopes: (u64, u64),
    /// How many of the romanization pass's values are kanji, and so need
    /// the download to read. Counted when the prompt was raised, since the
    /// answer needs the whole backlog; zero for every other pass.
    kanji: u64,
    /// Whether a switch the host just flipped is standing behind this. Cancel
    /// then means the switch was a no as well, and the host is told.
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
            // Both tempo prompts write the one worker count: it's the tempo
            // pass's setting, and which half of the library it's pointed at
            // doesn't change how many workers this machine wants.
            Pass::Tempo { .. } => Settings::update(move |s| s.tempo_workers = workers),
            // Nothing to write: the sort-name pass runs one worker
            // whatever the machine has, because MusicBrainz's rate limit
            // is the pace and a second worker would sleep through it.
            Pass::SortNames { .. } => {}
            // Nothing to write either: the pass is one pass over values in
            // memory and a batch of small writes, and splitting that over
            // workers would buy a second or two on the longest library.
            Pass::Romanize => {}
        }
    }

    /// Whether the worker slider means anything for this pass. It doesn't
    /// for the sort-name fill, and a slider that moves an estimate it
    /// can't change would be a lie in the one dialog that exists to be
    /// honest about cost.
    fn takes_workers(&self) -> bool {
        !matches!(self.pass, Pass::SortNames { .. } | Pass::Romanize)
    }

    /// What the pass will leave behind, or None when it gets through
    /// everything. Only the romanization pass has one: without the
    /// Japanese dictionary its kanji values are skipped and every other
    /// value still runs, which is a note rather than a wall. It was a wall
    /// once, and a dimmed Romanize button over a backlog that was nine
    /// tenths runnable read as broken.
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

/// Every worker the machine has. The prompt is a choice made in front of an
/// estimate, so the ceiling is the machine's rather than one a window picked
/// on the user's behalf.
pub fn cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Raise the prompt for a pass: count what it would get through, and read
/// back what the last one cost here. Clears whatever the last probe
/// reported, so a failure from one visit doesn't show up on the next.
pub fn raise<V: Host>(this: &mut V, pass: Pass, library: Entity<Library>, cx: &mut Context<V>) {
    let settings = Settings::load();
    let source = rox_services::acoustic::acoustic_source();
    // Both sort-name scopes at once: one walk of the symbol tables
    // answers both, and the dialog switches between them without asking
    // the library again.
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
    // The romanization backlog is one walk of the library, and both the
    // count and whether it needs a download come out of it.
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
        // The retry's count is the refused pile, not the missing one: those
        // are the tracks it would decode, and pricing it off `missing` would
        // quote a wait for work this run isn't doing.
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
        // The rate limit is the pace, so this is the one pass that opens
        // with a real estimate and never offers a probe. One worker, and
        // the slider that would move it isn't drawn.
        Pass::SortNames { scope } => (
            match scope {
                sortnames_job::Scope::NonLatin => sort_scopes.0,
                sortnames_job::Scope::All => sort_scopes.1,
            },
            sortnames_job::PACE,
            1,
        ),
        // Priced off a measured pace like the three analysis passes, not
        // off a constant like the fill: nothing external sets this one's
        // speed, so what it costs is what this machine costs, and that
        // swings by an order of magnitude between a library of kana and
        // one of kanji.
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
        Pass::Tempo { retry_refused } => {
            tempo_job::start(prompt.library.clone(), retry_refused, cx)
        }
        Pass::SortNames { scope } => sortnames_job::start(prompt.library.clone(), scope, cx),
        Pass::Romanize => romanize_job::start(prompt.library.clone(), cx),
    }
    this.pass_changed(cx);
    // The pass outlives whichever window started it, so hand the user
    // something that does too: the tasks window shows the count, the
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
    // The romanization probe reads values rather than files, so its sample
    // comes off the projection here rather than out of the database in the
    // background. A hundred is enough to average one slow segmentation
    // out; see `romanize_job::measure_pace`.
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
                    // The extractor always resolves, so this is the case
                    // that can't happen rather than one worth a message.
                    (Pass::Acoustic, None) => Err("no extractor to time".to_string()),
                    (Pass::ReplayGain, _) => {
                        replaygain_job::measure_pace(&db_path).map(Measured::ReplayGain)
                    }
                    // Timed over the pile this prompt would run, so the
                    // retry's estimate samples refusals rather than the
                    // tracks it isn't going to touch.
                    (Pass::Tempo { retry_refused }, _) => {
                        tempo_job::measure_pace(&db_path, retry_refused).map(Measured::Tempo)
                    }
                    // Unreachable in practice: the sort-name prompt always
                    // has an estimate, so the button that gets here is
                    // never drawn for it.
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
    Romanize(f32),
}

impl Measured {
    /// Worker-seconds per track, whichever pass it came from.
    fn pace(&self) -> f32 {
        match self {
            Measured::Acoustic(_, pace)
            | Measured::ReplayGain(pace)
            | Measured::Tempo(pace)
            | Measured::Romanize(pace) => *pace,
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
        Measured::Romanize(_) => Settings::update(move |s| s.session.romanize_pace = pace),
    }
}

/// Everything about the dialog that depends on which pass it's offering,
/// resolved once so the dialog itself is built one way.
struct Copy {
    title: SharedString,
    body: SharedString,
    action: SharedString,
}

fn copy(prompt: &Prompt) -> Copy {
    match prompt.pass {
        Pass::Acoustic => {
            // Tags mode rewrites the audio files, which is not something to
            // learn about afterwards, and it can't handle every format,
            // which is not something to work out from a coverage number.
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
            // Where the numbers get written is worth saying here: tags mode rewrites
            // the audio files, which is not something to learn about
            // afterwards.
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
        // The retry is the one prompt that offers to redo work rox already
        // did, so it says so: the count is tracks that were listened to and
        // came back with nothing, and hearing a beat this time means the
        // counting itself changed underneath them.
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
        // The one pass that talks to a service, so the body says whose
        // service and that nothing it finds is written into a file.
        Pass::SortNames { .. } => Copy {
            title: rox_i18n::t!("pass-sortnames-title", count = prompt.missing),
            body: rox_i18n::t!("pass-sortnames-body"),
            action: rox_i18n::t!("pass-fill"),
        },
        // The one pass that reads rather than asks, so the body says what
        // it's reading and that the guess is a guess.
        Pass::Romanize => Copy {
            title: rox_i18n::t!("pass-romanize-title", count = prompt.missing),
            body: rox_i18n::t!("pass-romanize-body"),
            action: rox_i18n::t!("pass-romanize"),
        },
    }
}

/// The prompt itself, or nothing while none is raised. The host drops this
/// at the root of its window body, over everything.
///
/// Same scrim and layering as the settings window's overwrite confirm, and
/// no click-away for the same reason: the buttons and the keyboard's Enter
/// and Escape are the ways out.
pub fn overlay<V: Host>(this: &V, window: &mut Window, cx: &mut Context<V>) -> Option<Div> {
    let prompt = this.prompt()?;
    let cores = cores();
    let copy = copy(prompt);
    let estimate = prompt.estimate();
    // A pass that will leave part of its backlog behind says so under the
    // estimate, rather than in place of it: it's still going to run, and
    // how long that takes is the line this dialog exists for.
    let shortfall = prompt.shortfall();
    // The estimate is the reason this dialog exists, so it says something
    // either way: the number, why there isn't one yet, or what went wrong
    // measuring it.
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
    // A probe that came back with nothing is the one case the line is bad
    // news, so it reads as a warning rather than as the estimate it stands
    // in for.
    let failed = estimate.is_none() && !prompt.probing && prompt.error.is_some();
    // Only offered while there's nothing measured: once there's a real
    // number, the pass itself keeps it honest and a second opinion off three
    // tracks would be the worse of the two.
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
    // The prompt holds the keyboard while it's asking, so Enter starts the
    // pass and Escape backs out from wherever focus was. Not once the focus
    // has moved inside it: Tab walks the prompt's own controls, and taking
    // it back every frame would pin it to the scrim.
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
                        // The buttons own Enter once one of them has focus; see
                        // the note on the focus above.
                        "enter" if this.dialog_focus().is_focused(window) => start(this, cx),
                        _ => return,
                    }
                    cx.stop_propagation();
                }),
            )
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
                                    .child(timing),
                            )
                            // Warn-toned, because it's work the person
                            // asked for that won't happen, and there's
                            // something they can do about it.
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

/// The sort-name pass's scope, where the worker slider would be: off is
/// the names a Latin reader can't file at all, on is every artist without
/// a sort name. A checkbox rather than two options, because the wide scope
/// is the narrow one plus the rest, and the estimate beneath it moves as
/// it's ticked, which is the whole argument the dialog is making.
///
/// Only for that pass; every other one returns nothing here.
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
                // Both counts were taken when the prompt was raised, so
                // the estimate reprices without another walk.
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

/// The worker slider's handler: the live count moves now, the file catches
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
            // last tick of a burst writes what the slider actually ended on.
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
