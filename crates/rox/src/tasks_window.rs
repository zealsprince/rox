//! The tasks window: one OS window listing the long library jobs, running
//! or not, so the settings window doesn't have to stay open to watch one.
//!
//! Four jobs are listed here. The library scan belongs to a workspace's
//! catalog; the acoustic pass ([`crate::embeddings`]), the ReplayGain measurement
//! ([`crate::replaygain_job`]) and the tempo pass ([`crate::tempo_job`]) are
//! app-global, outliving the window that started them. All three were
//! unwatchable once their page was closed: no count, no estimate, and no
//! way to stop short of reopening whatever started them. This is that
//! missing half.
//!
//! Those four rows are always there, idle or not. A list that only exists
//! while something is running is a progress bar with extra steps; this one
//! is also the answer to "what can I set going, and what would it cost",
//! which is the question someone opens it with before they've started
//! anything.
//!
//! Dynamic jobs are the other kind. They're started somewhere else (the
//! Last.fm loved-tracks import from the settings window, a conversion from
//! the convert dialog), they're measured in seconds or minutes rather than
//! afternoons, and there's nothing to say about them before someone sets
//! one going. Those rows appear when one runs and stay
//! for the session to report what it did, rather than standing in the list
//! saying nothing for the rest of the time.
//!
//! The scan keeps its menubar badge exactly as it was. The badge is a glance
//! and this is the detail: the same walk with the estimate and the file under
//! the cursor that never fit up there.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    div, prelude::*, px, relative, size, AnyElement, App, Bounds, Context, Div, Entity, EntityId,
    FocusHandle, Global, ScrollHandle, SharedString, Stateful, Subscription, WeakEntity, Window,
    WindowHandle,
};
use gpui_component::scroll::Scrollbar;
use gpui_component::Root;

use crate::lastfm::import;
use crate::{bake, convert, embeddings, pass_prompt, replaygain_job, tempo_job};
use rox_core::settings::{LayoutSize, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel;
use rox_panel_kit::ui as settings_ui;
use rox_services::catalog::{Library, LibraryEvent, ScanStatus};

/// How often a running pass repaints the surfaces watching it. Slower than
/// the settings page's own poll: this one redraws every window,
/// and a pass is measured in hours, so twice a second is plenty to read a
/// count by and cheap next to what the pass itself is doing.
const TICK: Duration = Duration::from_millis(500);

/// Repaint every window while a pass runs, and once more when it stops.
///
/// Called by both passes when they start. Without it the menubar chip would
/// be drawn once and then stay at whatever count it happened to catch: the
/// passes are app-global with no entity to observe, so nothing tells a
/// workspace that the number moved. One ticker for every surface rather
/// than a timer per surface, which also lets the tasks window get away with
/// no poll of its own.
///
/// The scan needs none of this: it belongs to a catalog entity that notifies
/// as it counts, and the tasks window observes that directly.
pub fn repaint_while_running(cx: &mut App) {
    // Every pass that repaints also belongs on the taskbar button, and that
    // sampler gates itself the same way this one does.
    crate::integrations::taskbar::watch(cx);
    // One ticker however many passes are running: both call this when they
    // start, and two tickers would just refresh the same windows twice as
    // often for the same picture.
    if cx.try_global::<Ticking>().is_some_and(|t| t.0) {
        return;
    }
    cx.set_global(Ticking(true));
    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(TICK).await;
        let live = cx.update(|cx| {
            // The falling edge repaints too, then ends the loop: the last
            // thing a pass does is stop, and that's the tick that swaps a
            // chip for nothing and a bar for a finished line.
            cx.refresh_windows();
            embeddings::progress(cx).is_some()
                || replaygain_job::progress(cx).is_some()
                || tempo_job::progress(cx).is_some()
                || import::progress(cx).is_some()
                || convert::progress(cx).is_some()
                || bake::progress(cx).is_some()
        });
        if !matches!(live, Ok(true)) {
            cx.update(|cx| cx.set_global(Ticking(false))).ok();
            break;
        }
    })
    .detach();
}

/// Whether a repaint ticker is already running, so the second pass to start
/// doesn't spawn one of its own.
#[derive(Default)]
struct Ticking(bool);

impl Global for Ticking {}

/// The menubar's tasks control: always a way into the window, showing what's
/// running while anything is.
///
/// Always drawn, because the window it opens is now worth opening with
/// nothing running: it's where the passes are started from. Idle it's a
/// plain icon, the same weight as the rescan button beside it; running it
/// grows into a chip with the count, which is the one thing worth reading
/// from the bar itself.
///
/// The library scan isn't in here: it has the badge and the
/// status line to its left, and saying it twice in one bar would be noise.
pub fn control<P: 'static>(cx: &mut Context<P>) -> Stateful<Div> {
    // More than one at a time is a count rather than a chip each: the bar is
    // shared with the catalog status and the scan controls, and the window is
    // one click away for the detail.
    let mut live: Vec<(&'static str, String)> = Vec::new();
    if let Some(job) = embeddings::progress(cx) {
        live.push((
            Job::Acoustic.icon(),
            rox_i18n::t!("tasks-analyzing", progress = share(job.done(), job.total())).to_string(),
        ));
    }
    if let Some(job) = replaygain_job::progress(cx) {
        live.push((
            Job::ReplayGain.icon(),
            rox_i18n::t!("tasks-measuring", progress = share(job.done(), job.total())).to_string(),
        ));
    }
    if let Some(job) = tempo_job::progress(cx) {
        live.push((
            Job::Tempo.icon(),
            rox_i18n::t!("tasks-timing", progress = share(job.done(), job.total())).to_string(),
        ));
    }
    if let Some(job) = import::progress(cx) {
        live.push((
            Job::LovedImport.icon(),
            rox_i18n::t!("tasks-importing", progress = share(job.done(), job.total())).to_string(),
        ));
    }
    if let Some(job) = convert::progress(cx) {
        live.push((
            Job::Convert.icon(),
            rox_i18n::t!(
                "tasks-converting",
                progress = share(job.done(), job.total())
            )
            .to_string(),
        ));
    }
    if let Some(job) = bake::progress(cx) {
        live.push((
            Job::Bake.icon(),
            rox_i18n::t!("tasks-embedding", progress = share(job.done(), job.total())).to_string(),
        ));
    }
    let running = match live.len() {
        0 => None,
        1 => live.pop(),
        several => Some((
            icons::CLOCK,
            rox_i18n::t!("tasks-chip-count", count = several as u64).to_string(),
        )),
    };
    let open = cx.listener(|_, _, _, cx| open(cx));
    // Idle the glyph is a clock and nothing else, so the tip is the only
    // thing that says what it opens. Running, the chip has the count and
    // the tip stays on the click.
    let tip = panel::Tip::keyed("tasks", rox_i18n::t!("tasks-tip"));
    let Some((path, label)) = running else {
        return tip.apply(
            div()
                .flex_none()
                .p(tokens::ICON_PAD)
                .rounded(tokens::RADIUS)
                .hover(|d| d.bg(palette::bg_control()))
                .cursor_pointer()
                .child(icon(icons::CLOCK))
                .on_mouse_down(gpui::MouseButton::Left, open),
        );
    };
    tip.apply(
        div()
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .gap(tokens::SPACE_XS)
            .px(tokens::SPACE_SM)
            .py(px(2.))
            .rounded_full()
            .bg(palette::bg_control())
            .text_xs()
            .text_color(palette::text_muted())
            .cursor_pointer()
            .hover(|d| d.bg(palette::bg_control_hover()))
            .child(icon(path))
            .child(SharedString::from(label))
            .on_mouse_down(gpui::MouseButton::Left, open),
    )
}

/// What the rest of a pass costs at the worker count it would run with, off
/// the pace the last one measured here. The worker count comes with it
/// because the estimate means nothing without it: the same library is four
/// hours or one depending on what it's allowed to use.
fn priced(pace: f32, missing: u64, workers: usize) -> Option<String> {
    let estimate = rox_core::pace::estimate(pace, missing, workers)?;
    Some(
        rox_i18n::t!(
            "tasks-estimate-at",
            estimate = estimate,
            workers = rox_core::pace::workers_phrase(workers)
        )
        .to_string(),
    )
}

/// How far along as a percentage, or an ellipsis while the work list is
/// still being built and there's nothing to be a percentage of.
fn share(done: usize, total: usize) -> String {
    if total == 0 {
        return "...".into();
    }
    rox_i18n::format::format_percent((done.min(total) * 100 / total).min(100) as f64)
}

/// The window's floor. Wide enough that a path and a count share a line
/// without wrapping, tall enough for the three rows at once.
const MIN: gpui::Size<gpui::Pixels> = gpui::Size {
    width: px(420.),
    height: px(320.),
};

/// The open tasks window, if any: opening again focuses it rather than
/// stacking a second one, the console window's move.
struct OpenTasks(WindowHandle<Root>);

impl Global for OpenTasks {}

/// Open the tasks window, or bring the open one to the front.
///
/// Deferred for the console window's reason: the callers that open it (a
/// menu action, the settings window starting a pass) are inside another
/// entity's update, and reading the front workspace for the tint mid-update
/// would panic.
pub fn open(cx: &mut App) {
    cx.defer(open_now);
}

fn open_now(cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenTasks>() {
        let handle = open.0;
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    // The catalog the scan row drives and the passes read comes from
    // whichever workspace is in front when the window opens, the same place
    // the tint does. It's held weakly from there on: a window that outlives
    // its workspace should go inert, not keep a dead one's library alive.
    let front = rox_panel_api::windows::front_workspace(cx).map(|(_, state)| state);
    let player = front.as_ref().map(|state| state.player.entity_id());
    let library = front.map(|state| state.library);
    let (width, height) = Settings::load()
        .windows
        .tasks
        .filter(|s| s.width >= f32::from(MIN.width) && s.height >= f32::from(MIN.height))
        .map(|s| (s.width, s.height))
        // Room for the standing rows and a dynamic one under them without a
        // scroll on first open.
        .unwrap_or((640., 480.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let handle = panel::open_child_window(
        cx,
        rox_i18n::t!("tasks-window-title"),
        bounds,
        Some(MIN),
        move |window, cx| cx.new(|cx| TasksWindow::new(player, library.clone(), window, cx)),
    );
    cx.set_global(OpenTasks(handle));
}

/// Which job a row is describing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Job {
    Scan,
    Acoustic,
    ReplayGain,
    /// The tempo pass ([`crate::tempo_job`]): what every track with no BPM
    /// runs at.
    Tempo,
    /// The dynamic one: Last.fm's loved tracks pulled in as hearts, started
    /// from the settings window rather than from here.
    LovedImport,
    /// The other dynamic one: a selection through ffmpeg into another
    /// format, started from the convert dialog. Selection-scoped, so it has
    /// nothing to say before someone picks tracks and a folder.
    Convert,
    /// Stored lyrics, gains and vectors written into the files themselves,
    /// started from the embed dialog. Nothing to say before someone has
    /// picked which of the three to write.
    Bake,
}

/// Top to bottom, cheapest first: a scan is minutes and the three passes are
/// afternoons, and the passes read what the scan writes. The dynamic jobs
/// fall in under these when they have anything to say.
const JOBS: [Job; 4] = [Job::Scan, Job::Acoustic, Job::ReplayGain, Job::Tempo];

impl Job {
    fn label(self) -> SharedString {
        match self {
            Job::Scan => rox_i18n::t!("tasks-job-scan"),
            Job::Acoustic => rox_i18n::t!("tasks-job-acoustic"),
            Job::ReplayGain => rox_i18n::t!("tasks-job-replaygain"),
            Job::Tempo => rox_i18n::t!("tasks-job-tempo"),
            Job::LovedImport => rox_i18n::t!("tasks-job-loved-import"),
            Job::Convert => rox_i18n::t!("tasks-job-convert"),
            Job::Bake => "Embed Stored Metadata".into(),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Job::Scan => icons::REFRESH_CW,
            Job::Acoustic => icons::FLASK,
            Job::ReplayGain => icons::GAUGE,
            // Beats a minute is a rate over time, and the clock is the one
            // glyph in the set that says so without borrowing the gauge the
            // measurement pass uses.
            Job::Tempo => icons::CLOCK,
            Job::LovedImport => icons::HEART,
            Job::Convert => icons::AUDIO_LINES,
            Job::Bake => icons::UPLOAD,
        }
    }

    /// What the start button says and shows, or None for a job this window
    /// only watches. The wording matches the settings page's buttons, since
    /// they start the same work.
    fn start_label(self) -> Option<(SharedString, &'static str)> {
        match self {
            Job::Scan => Some((rox_i18n::t!("tasks-start-rescan"), icons::REFRESH_CW)),
            Job::Acoustic => Some((rox_i18n::t!("tasks-start-analyze-missing"), icons::FLASK)),
            Job::ReplayGain => Some((rox_i18n::t!("tasks-start-measure-missing"), icons::GAUGE)),
            Job::Tempo => Some((rox_i18n::t!("tasks-start-analyze-missing"), icons::CLOCK)),
            // The import belongs to an account, not to a library, and it
            // reads its user off the settings it's started from. Offering
            // it here would be a second door into a room with one chair.
            Job::LovedImport => None,
            // A conversion is a selection, a format and a folder. None of
            // those exist here, so this row only ever watches.
            Job::Convert => None,
            // An embed is three counts and three checkboxes, and the counts
            // take a survey to work out. That's a dialog, not a button.
            Job::Bake => None,
        }
    }

    /// Ask a running job to stop. Only the scan needs the catalog to say it
    /// to; the rest hold their own cancel flag, so a workspace closed under
    /// them is no reason to have to wait one out.
    fn stop(self, library: Option<&Entity<Library>>, cx: &mut App) {
        match self {
            Job::Scan => {
                if let Some(library) = library {
                    library.update(cx, |library, cx| library.abort_scan(cx));
                }
            }
            Job::Acoustic => embeddings::stop(cx),
            Job::ReplayGain => replaygain_job::stop(cx),
            Job::Tempo => tempo_job::stop(cx),
            Job::LovedImport => import::stop(cx),
            Job::Convert => convert::stop(cx),
            Job::Bake => bake::stop(cx),
        }
    }
}

/// One running job, flattened out of whichever of the three it came from.
/// The three progress types are unrelated and cover the same questions, so
/// the row below is written once against this rather than three times.
struct Snapshot {
    done: usize,
    total: usize,
    failed: usize,
    current: String,
    /// Whether `current` is a file path, so the readout shows its name
    /// rather than the whole line. The passes step through files; the import
    /// steps through track names, which are already what to show.
    current_is_path: bool,
    eta: Option<f64>,
    stopping: bool,
}

impl Snapshot {
    fn acoustic(job: &rox_acoustic::Progress) -> Snapshot {
        Snapshot {
            done: job.done(),
            total: job.total(),
            failed: job.failed(),
            current: job.current(),
            current_is_path: true,
            eta: job.eta_secs(),
            stopping: job.stopping(),
        }
    }

    /// The import counts loved tracks read, and the ones it couldn't place
    /// are its failed count: nothing went wrong with them, this library
    /// just has no home for them.
    fn import(job: &import::Progress) -> Snapshot {
        Snapshot {
            done: job.done(),
            total: job.total(),
            failed: job.unmatched(),
            current: job.current(),
            current_is_path: false,
            eta: job.eta_secs(),
            stopping: job.stopping(),
        }
    }

    /// A conversion counts files, and the ones ffmpeg refused are its
    /// failed count. What the plan skipped before the run started isn't in
    /// here: those never became work, and the finished line reports them.
    fn convert(job: &convert::Progress) -> Snapshot {
        Snapshot {
            done: job.done(),
            total: job.total(),
            failed: job.failed(),
            current: job.current(),
            current_is_path: true,
            eta: job.eta_secs(),
            stopping: job.stopping(),
        }
    }

    /// An embed counts files, and the ones the writer refused are its failed
    /// count. What the survey refused isn't in here: those never became work,
    /// and the finished line reports them.
    fn bake(job: &bake::Progress) -> Snapshot {
        Snapshot {
            done: job.done(),
            total: job.total(),
            failed: job.failed(),
            current: job.current(),
            current_is_path: true,
            eta: job.eta_secs(),
            stopping: job.stopping(),
        }
    }

    /// The tempo pass counts tracks, and the ones it looked at without
    /// getting an answer are its failed count: a file that wouldn't decode
    /// and one whose beat the estimator refused to call both leave the row
    /// as it was.
    fn tempo(job: &tempo_job::Progress) -> Snapshot {
        Snapshot {
            done: job.done(),
            total: job.total(),
            failed: job.failed(),
            current: job.current(),
            current_is_path: true,
            eta: job.eta_secs(),
            stopping: job.stopping(),
        }
    }

    fn replaygain(job: &replaygain_job::Progress) -> Snapshot {
        Snapshot {
            done: job.done(),
            total: job.total(),
            failed: job.failed(),
            current: job.current(),
            current_is_path: true,
            eta: job.eta_secs(),
            stopping: job.stopping(),
        }
    }

    /// A scan counts files it went through, not files it gave up on: an
    /// unreadable one stays in the library rather than being skipped, so
    /// there's no failed count to report.
    fn scan(scan: ScanStatus) -> Snapshot {
        Snapshot {
            done: scan.done,
            total: scan.total,
            failed: 0,
            current: scan.current,
            current_is_path: true,
            eta: scan.eta,
            stopping: scan.stopping,
        }
    }
}

/// How far along everything running is, as (done, total) summed over the
/// jobs, or None when nothing is running at all.
///
/// One number for the batch is all a taskbar button can draw, and summing
/// beats averaging: every job here counts files, so two half-done passes
/// read as half done rather than as a fraction of a fraction. A job still
/// working out its list adds nothing to either side: it hasn't got a total
/// yet.
///
/// Read live off the same four sources the rows use, so this works with
/// no window open.
pub(crate) fn aggregate(cx: &mut App) -> Option<(usize, usize)> {
    // The scan belongs to a catalog rather than the app, so it comes off
    // whichever workspace is in front, the same place this window takes it
    // from when it opens.
    let library = rox_panel_api::windows::front_workspace(cx).map(|(_, state)| state.library);
    let scan = library.and_then(|library| library.read(cx).scan_status());
    let mut running: Vec<Snapshot> = Vec::new();
    running.extend(scan.map(Snapshot::scan));
    running.extend(embeddings::progress(cx).as_deref().map(Snapshot::acoustic));
    running.extend(
        replaygain_job::progress(cx)
            .as_deref()
            .map(Snapshot::replaygain),
    );
    running.extend(tempo_job::progress(cx).as_deref().map(Snapshot::tempo));
    running.extend(import::progress(cx).as_deref().map(Snapshot::import));
    running.extend(convert::progress(cx).as_deref().map(Snapshot::convert));
    running.extend(bake::progress(cx).as_deref().map(Snapshot::bake));
    if running.is_empty() {
        return None;
    }
    Some(running.iter().fold((0, 0), |(done, total), job| {
        (done + job.done.min(job.total), total + job.total)
    }))
}

/// What a pass left behind when it stopped running, so its row can still say
/// what happened rather than going quiet the moment it finishes.
#[derive(Clone)]
struct Finished {
    done: usize,
    failed: usize,
    /// Whether it was asked to stop rather than reaching the end. A pass that
    /// was stopped and a pass that finished have the same counts and very
    /// different meanings.
    stopped: bool,
}

impl Finished {
    fn line(&self) -> String {
        let mut line = if self.stopped {
            rox_i18n::t!("tasks-last-run-stopped", count = self.done as u64).to_string()
        } else {
            rox_i18n::t!("tasks-last-run-finished", count = self.done as u64).to_string()
        };
        if self.failed > 0 {
            line.push_str(&format!(
                " {}",
                rox_i18n::t!("tasks-failed-suffix", count = self.failed as u64)
            ));
        }
        line
    }
}

/// Why a row can't start, with what the row should say about it, or
/// None where the idle line above already covers it.
struct Blocked(Option<SharedString>);

/// The three app-global passes as of the last poll. The scan isn't in here:
/// it's in the catalog, which is asked for it when a row is drawn.
#[derive(Default)]
struct Live {
    acoustic: Option<Arc<rox_acoustic::Progress>>,
    replaygain: Option<Arc<replaygain_job::Progress>>,
    tempo: Option<Arc<tempo_job::Progress>>,
}

/// What the idle rows state about the library, and what it costs to find
/// out: every field here is a pass over the tracks table or a read of the
/// settings file. Re-read when the library says something changed and when a
/// pass ends, never per frame.
#[derive(Default)]
struct Facts {
    /// Folders the library scans, for the scan row's idle line.
    roots: usize,
    /// When the last full scan finished, in unix seconds; 0 for never.
    last_scan: i64,
    /// The acoustic model's name and how much of the library it describes.
    /// Per model: every model describes the library separately, and a bare
    /// count would read as the library's own.
    acoustic_label: String,
    acoustic: rox_library::embeddings::Coverage,
    /// Whether describing tracks is switched on at all. The pass no-ops
    /// while it's off, so the row says so rather than offering a button
    /// that does nothing.
    acoustic_on: bool,
    /// Roughly what analyzing the rest would cost here, off the pace the
    /// last pass measured on this machine. None until one has run.
    acoustic_estimate: Option<String>,
    /// Tracks with no gain from either their tags or a measurement, and the
    /// whole count they're out of.
    rg_missing: u64,
    rg_total: u64,
    rg_estimate: Option<String>,
    /// The library's tempo split, tagged against measured against neither.
    bpm: rox_library::store::BpmCoverage,
    /// Whether the tempo pass is switched on at all. It no-ops while it's
    /// off, so the row says so rather than offering a dead button.
    tempo_on: bool,
    tempo_estimate: Option<String>,
}

struct TasksWindow {
    /// The workspace player the window themes to, if one was up when it
    /// opened; None themes to the base palette.
    player: Option<EntityId>,
    /// The catalog the scan row drives and both passes read. Weak: the
    /// window outlives its workspace, and a dead library leaves the rows
    /// readable but inert rather than taking the window with it.
    library: Option<WeakEntity<Library>>,
    /// Live scan counts and the coverage refresh, off the catalog itself.
    _subs: Vec<Subscription>,
    /// The running passes as of the last poll.
    live: Live,
    /// What each pass left when it stopped, kept so the row can report.
    acoustic_done: Option<Finished>,
    replaygain_done: Option<Finished>,
    tempo_done: Option<Finished>,
    facts: Facts,
    /// The start prompt, while one is up: the same dialog the settings page
    /// raises, with the worker slider and the estimate.
    prompt: Option<pass_prompt::Prompt>,
    /// The prompt slider's click-to-type state. One per window, since only
    /// one value is ever being typed into.
    value_edit: panel::ValueEdit,
    /// The keyboard's home while the prompt is up, so Enter and Escape reach
    /// it.
    dialog_focus: FocusHandle,
    /// The window's own focus, claimed on open. Not a tab stop itself, so
    /// the first Tab moves to the first control; it's here because a window
    /// holding focus nowhere never sees a key.
    focus: FocusHandle,
    /// The row list's scroll position, shared with the scrollbar.
    scroll: ScrollHandle,
}

/// The pass prompt's host side. The window caches counts, so a start or a
/// probe means re-reading them.
impl pass_prompt::Host for TasksWindow {
    fn prompt(&self) -> Option<&pass_prompt::Prompt> {
        self.prompt.as_ref()
    }

    fn prompt_mut(&mut self) -> &mut Option<pass_prompt::Prompt> {
        &mut self.prompt
    }

    fn value_edit(&self) -> &panel::ValueEdit {
        &self.value_edit
    }

    fn dialog_focus(&self) -> &FocusHandle {
        &self.dialog_focus
    }

    fn pass_changed(&mut self, cx: &mut Context<Self>) {
        self.read_facts(cx);
        cx.notify();
    }
}

impl TasksWindow {
    fn new(
        player: Option<EntityId>,
        library: Option<Entity<Library>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // The frame persists on the OS close button, which never runs
        // remove_window, so write the size in the should-close hook, the
        // console window's move.
        window.on_window_should_close(cx, |window, _| {
            let frame = window.window_bounds().get_bounds();
            Settings::update(move |s| {
                s.windows.tasks = Some(LayoutSize {
                    width: frame.size.width.into(),
                    height: frame.size.height.into(),
                });
            });
            true
        });
        // No poll of its own for the passes: [`repaint_while_running`]
        // redraws every window while one is going and once more when it
        // stops, so this window resamples in render and is live for free.
        // The scan comes off the catalog instead, which notifies as it
        // counts; the same entity says when the counts behind the idle rows
        // have moved.
        let subs = library
            .as_ref()
            .map(|library| {
                vec![
                    cx.observe(library, |_, _, cx| cx.notify()),
                    cx.subscribe(library, |this, _, event, cx| {
                        if matches!(event, LibraryEvent::Updated) {
                            this.read_facts(cx);
                            cx.notify();
                        }
                    }),
                ]
            })
            .unwrap_or_default();
        let focus = cx.focus_handle();
        window.focus(&focus);
        let mut this = TasksWindow {
            player,
            library: library.map(|library| library.downgrade()),
            _subs: subs,
            live: Live::default(),
            acoustic_done: None,
            replaygain_done: None,
            tempo_done: None,
            facts: Facts::default(),
            prompt: None,
            value_edit: panel::ValueEdit::default(),
            dialog_focus: cx.focus_handle(),
            focus: focus.clone(),
            scroll: ScrollHandle::new(),
        };
        this.read_facts(cx);
        this.sample(cx);
        this
    }

    /// The catalog behind the rows, while its workspace is still open.
    fn library(&self) -> Option<Entity<Library>> {
        self.library.as_ref().and_then(|library| library.upgrade())
    }

    /// Re-read what the idle rows state. Two aggregate queries and a settings
    /// read, so this runs on the events that can have moved them, never per
    /// frame.
    fn read_facts(&mut self, cx: &mut Context<Self>) {
        let Some(library) = self.library() else {
            return;
        };
        let settings = Settings::load();
        let source = rox_services::acoustic::acoustic_source();
        let library = library.read(cx);
        let acoustic = library.acoustic_coverage(source.id());
        let gains = library.replaygain_breakdown();
        let bpm = library.bpm_breakdown();
        self.facts =
            Facts {
                roots: library.roots().len(),
                last_scan: settings.session.last_scan,
                acoustic_label: source.label(),
                acoustic,
                acoustic_on: settings.acoustic_analysis,
                acoustic_estimate: settings.session.acoustic_pace.get(source.id()).and_then(
                    |pace| priced(*pace, acoustic.missing() as u64, settings.acoustic_workers),
                ),
                rg_missing: gains.missing,
                rg_total: gains.total(),
                rg_estimate: priced(
                    settings.session.replaygain_pace,
                    gains.missing,
                    settings.replaygain_workers,
                ),
                bpm,
                tempo_on: settings.tempo_analysis,
                tempo_estimate: priced(
                    settings.session.tempo_pace,
                    bpm.missing,
                    settings.tempo_workers,
                ),
            };
    }

    /// Re-read both pass globals, remembering the final counts of whichever
    /// one just stopped, and picking the new counts up with it.
    fn sample(&mut self, cx: &mut Context<Self>) {
        let mut ended = false;
        if let Some(job) = self.live.acoustic.take() {
            if embeddings::progress(cx).is_none() {
                self.acoustic_done = Some(Finished {
                    done: job.done(),
                    failed: job.failed(),
                    stopped: job.stopping(),
                });
                ended = true;
            }
        }
        if let Some(job) = self.live.replaygain.take() {
            if replaygain_job::progress(cx).is_none() {
                self.replaygain_done = Some(Finished {
                    done: job.done(),
                    failed: job.failed(),
                    stopped: job.stopping(),
                });
                ended = true;
            }
        }
        if let Some(job) = self.live.tempo.take() {
            if tempo_job::progress(cx).is_none() {
                self.tempo_done = Some(Finished {
                    done: job.done(),
                    failed: job.failed(),
                    stopped: job.stopping(),
                });
                ended = true;
            }
        }
        self.live.acoustic = embeddings::progress(cx);
        self.live.replaygain = replaygain_job::progress(cx);
        self.live.tempo = tempo_job::progress(cx);
        // A pass that started again clears what the last one left, so the
        // row never shows a finished line under a running bar.
        if self.live.acoustic.is_some() {
            self.acoustic_done = None;
        }
        if self.live.replaygain.is_some() {
            self.replaygain_done = None;
        }
        if self.live.tempo.is_some() {
            self.tempo_done = None;
        }
        // A pass that just ended moved the count its own row states. The
        // scan's finish comes through the library's event instead.
        if ended {
            self.read_facts(cx);
        }
    }

    /// The running job behind a row, if it's running.
    fn running(&self, job: Job, cx: &App) -> Option<Snapshot> {
        match job {
            Job::Scan => self
                .library()
                .and_then(|library| library.read(cx).scan_status())
                .map(Snapshot::scan),
            Job::Acoustic => self.live.acoustic.as_ref().map(|j| Snapshot::acoustic(j)),
            Job::ReplayGain => self
                .live
                .replaygain
                .as_ref()
                .map(|j| Snapshot::replaygain(j)),
            Job::Tempo => self.live.tempo.as_ref().map(|j| Snapshot::tempo(j)),
            // Read live rather than off the poll: the import is seconds
            // long, so a sample taken a frame ago is a sample of a
            // different job.
            Job::LovedImport => import::progress(cx).as_deref().map(Snapshot::import),
            // Read live for the import's reason: a conversion is minutes
            // at most, so a sample from a frame ago is a sample of a
            // different job.
            Job::Convert => convert::progress(cx).as_deref().map(Snapshot::convert),
            // Read live for the same reason: an embed is a commit per file
            // and often over in seconds.
            Job::Bake => bake::progress(cx).as_deref().map(Snapshot::bake),
        }
    }

    /// The jobs that only exist while something is happening. Started
    /// elsewhere, so there's nothing to say about one before it runs and no
    /// row for it either; once it has run, its row stays for the session
    /// with what it did, the same as the standing rows report their last
    /// pass.
    fn dynamic(&self, cx: &App) -> Vec<Job> {
        let import = import::progress(cx).is_some() || import::last(cx).is_some();
        let convert = convert::progress(cx).is_some() || convert::last(cx).is_some();
        let bake = bake::progress(cx).is_some() || bake::last(cx).is_some();
        import
            .then_some(Job::LovedImport)
            .into_iter()
            .chain(convert.then_some(Job::Convert))
            .chain(bake.then_some(Job::Bake))
            .collect()
    }

    /// What an idle row says: where the library stands on this job, and what
    /// the rest of it would cost. One line per thing worth knowing, so a row
    /// with nothing to add stays one line tall.
    fn idle_lines(&self, job: Job, cx: &App) -> Vec<String> {
        let mut lines = Vec::new();
        match job {
            Job::Scan => {
                // The library's own status line, which after a scan is that
                // scan's rollup and otherwise the track count.
                let status = self
                    .library()
                    .map(|library| library.read(cx).status().to_string())
                    .unwrap_or_default();
                if !status.is_empty() {
                    lines.push(status);
                }
                if self.facts.roots == 0 {
                    lines.push(rox_i18n::t!("tasks-scan-no-folders").to_string());
                } else {
                    let folders =
                        rox_i18n::t!("tasks-scan-folder-count", count = self.facts.roots as u64)
                            .to_string();
                    lines.push(match self.since_scan() {
                        Some(ago) => rox_i18n::t!(
                            "tasks-scan-last-scanned",
                            folders = folders.clone(),
                            ago = ago
                        )
                        .to_string(),
                        None => {
                            rox_i18n::t!("tasks-scan-never-scanned", folders = folders).to_string()
                        }
                    });
                }
            }
            Job::Acoustic => {
                let coverage = self.facts.acoustic;
                let label = &self.facts.acoustic_label;
                if !self.facts.acoustic_on {
                    lines.push(rox_i18n::t!("tasks-acoustic-off").to_string());
                } else if coverage.total == 0 {
                    lines.push("Nothing scanned to analyze yet".into());
                } else if coverage.missing() == 0 {
                    lines.push(
                        rox_i18n::t!(
                            "tasks-acoustic-all-described",
                            count = coverage.total as u64,
                            label = label.clone()
                        )
                        .to_string(),
                    );
                } else {
                    let mut line = rox_i18n::t!(
                        "tasks-acoustic-partial",
                        label = label.clone(),
                        embedded = coverage.embedded as u64,
                        total = coverage.total as u64
                    )
                    .to_string();
                    if let Some(estimate) = &self.facts.acoustic_estimate {
                        line.push_str(&rox_i18n::t!(
                            "tasks-rest-takes",
                            estimate = estimate.clone()
                        ));
                    }
                    lines.push(line);
                }
                if let Some(reason) = embeddings::last_failure(cx) {
                    lines
                        .push(rox_i18n::t!("tasks-last-pass-stopped", reason = reason).to_string());
                }
                if let Some(done) = &self.acoustic_done {
                    lines.push(done.line());
                }
            }
            Job::ReplayGain => {
                if self.facts.rg_total == 0 {
                    lines.push(rox_i18n::t!("tasks-nothing-to-measure").to_string());
                } else if self.facts.rg_missing == 0 {
                    lines.push(
                        rox_i18n::t!("tasks-rg-all-gain", count = self.facts.rg_total).to_string(),
                    );
                } else {
                    let mut line = rox_i18n::t!(
                        "tasks-rg-partial",
                        missing = self.facts.rg_missing,
                        total = self.facts.rg_total
                    )
                    .to_string();
                    if let Some(estimate) = &self.facts.rg_estimate {
                        line.push_str(&rox_i18n::t!(
                            "tasks-measuring-takes",
                            estimate = estimate.clone()
                        ));
                    }
                    lines.push(line);
                }
                if let Some(done) = &self.replaygain_done {
                    lines.push(done.line());
                }
            }
            Job::Tempo => {
                let bpm = self.facts.bpm;
                if !self.facts.tempo_on {
                    lines.push(rox_i18n::t!("tasks-tempo-off").to_string());
                } else if bpm.total() == 0 {
                    lines.push("Nothing scanned to analyze yet".into());
                } else if bpm.missing == 0 {
                    lines.push(rox_i18n::t!("tasks-tempo-all", count = bpm.total()).to_string());
                } else {
                    let mut line = rox_i18n::t!(
                        "tasks-tempo-partial",
                        missing = bpm.missing,
                        total = bpm.total()
                    )
                    .to_string();
                    if let Some(estimate) = &self.facts.tempo_estimate {
                        line.push_str(&rox_i18n::t!(
                            "tasks-working-out-takes",
                            estimate = estimate.clone()
                        ));
                    }
                    lines.push(line);
                }
                if let Some(done) = &self.tempo_done {
                    lines.push(done.line());
                }
            }
            Job::LovedImport => match import::last(cx) {
                Some(Ok(summary)) => {
                    lines.push(summary.line());
                    if summary.unmatched > 0 {
                        lines.push(
                            rox_i18n::t!(
                                "tasks-import-unmatched",
                                count = summary.unmatched as u64
                            )
                            .to_string(),
                        );
                    }
                }
                Some(Err(e)) => lines
                    .push(rox_i18n::t!("tasks-import-failed", error = e.to_string()).to_string()),
                // Only reachable for a frame, between the row appearing and
                // the first progress arriving.
                None => lines.push(rox_i18n::t!("tasks-import-reading").to_string()),
            },
            Job::Convert => {
                match convert::last(cx) {
                    Some(summary) => lines.push(summary.line()),
                    // Only reachable for a frame, between the row appearing
                    // and the first file finishing.
                    None => lines.push(rox_i18n::t!("tasks-convert-starting").to_string()),
                }
                // ffmpeg's own error for the first file it refused. A count
                // with no reason sends someone to the log.
                if let Some(reason) = convert::last_failure(cx) {
                    lines.push(reason);
                }
            }
            Job::Bake => {
                match bake::last(cx) {
                    Some(summary) => lines.push(summary.line()),
                    // Only reachable for a frame, between the row appearing
                    // and the first file being written.
                    None => lines.push(rox_i18n::t!("tasks-bake-writing").to_string()),
                }
                // The writer's error for the first file it refused. A count
                // with no reason sends someone to the log.
                if let Some(reason) = bake::last_failure(cx) {
                    lines.push(reason);
                }
            }
        }
        lines
    }

    /// How long ago the last full scan finished, or None if none ever has.
    fn since_scan(&self) -> Option<String> {
        if self.facts.last_scan <= 0 {
            return None;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Some(rox_core::pace::human(
            now.saturating_sub(self.facts.last_scan).max(0) as f64,
        ))
    }

    /// Why a row's start button is inert, if it is, and whether that's worth
    /// saying out loud. Nothing to start is usually the line above already:
    /// "nothing left to analyze" under "all 12,000 tracks are described" is
    /// the row saying the same thing twice. The reason that will pass earns
    /// a line, since that one is worth waiting out.
    fn blocked(&self, job: Job, cx: &App) -> Option<Blocked> {
        // A watched job has no start button to explain the state of, and the
        // import doesn't touch the rows a scan rewrites anyway.
        job.start_label()?;
        let library = self.library()?;
        // Anything the library is already doing blocks all three: a scan
        // rewrites the very rows the passes read, and the catalog runs one
        // refresh at a time anyway.
        if library.read(cx).busy().is_some() {
            return Some(Blocked(Some(if library.read(cx).scanning() {
                rox_i18n::t!("tasks-library-scanning")
            } else {
                rox_i18n::t!("tasks-library-busy")
            })));
        }
        match job {
            Job::Scan => (!library.read(cx).can_rescan()).then_some(Blocked(None)),
            Job::Acoustic => {
                if !self.facts.acoustic_on || self.facts.acoustic.missing() == 0 {
                    Some(Blocked(None))
                } else {
                    // The pass would load a half-written model file, and the
                    // download has to finish first anyway.
                    embeddings::models::progress(cx)
                        .map(|_| Blocked(Some(rox_i18n::t!("tasks-model-downloading"))))
                }
            }
            Job::ReplayGain => (self.facts.rg_missing == 0).then_some(Blocked(None)),
            Job::Tempo => {
                (!self.facts.tempo_on || self.facts.bpm.missing == 0).then_some(Blocked(None))
            }
            // Returned above; a watched job never reaches here.
            Job::LovedImport | Job::Convert | Job::Bake => None,
        }
    }

    /// One job: what it is and its button, then either a bar and a live count
    /// or where the library stands on it.
    fn row(&self, job: Job, cx: &mut Context<Self>) -> Div {
        let running = self.running(job, cx);
        let blocked = running.is_none().then(|| self.blocked(job, cx)).flatten();
        card()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(tokens::SPACE_SM)
                    .child(icon(job.icon()))
                    .child(div().flex_1().child(job.label()))
                    .children(self.button(job, running.as_ref(), blocked.is_some(), cx))
                    .children(self.dismiss(job, running.is_some(), cx)),
            )
            .map(|d| match &running {
                Some(snapshot) => d.children(self.running_lines(snapshot)),
                None => d
                    .children(self.idle_lines(job, cx).into_iter().map(muted))
                    .children(
                        blocked
                            .and_then(|blocked| blocked.0)
                            .map(|why| muted(why.to_string())),
                    ),
            })
    }

    /// A running job's readout: the bar, the count and estimate under it, and
    /// the file it's on.
    fn running_lines(&self, snapshot: &Snapshot) -> Vec<Div> {
        // Zero total means the work list is still being built, which is a
        // real state a big library stays in for a second or two.
        let counted = snapshot.done.min(snapshot.total);
        let fraction = if snapshot.total == 0 {
            0.0
        } else {
            counted as f32 / snapshot.total as f32
        };
        let mut line = if snapshot.total == 0 {
            rox_i18n::t!("tasks-working-out-missing").to_string()
        } else {
            rox_i18n::t!(
                "tasks-count-of-total",
                done = counted as u64,
                total = snapshot.total as u64
            )
            .to_string()
        };
        if let Some(eta) = snapshot.eta {
            line.push_str(&rox_i18n::t!(
                "tasks-time-left",
                left = rox_core::pace::human(eta)
            ));
        }
        if snapshot.failed > 0 {
            line.push_str(&format!(
                " {}",
                rox_i18n::t!("tasks-failed-suffix", count = snapshot.failed as u64)
            ));
        }
        let mut lines = vec![bar(fraction), muted(line)];
        let current = if snapshot.current_is_path {
            std::path::Path::new(&snapshot.current)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        } else {
            Some(snapshot.current.clone()).filter(|line| !line.is_empty())
        };
        lines.extend(current.map(muted));
        lines
    }

    /// A row's one control: stop what's running, or start what isn't.
    fn button(
        &self,
        job: Job,
        running: Option<&Snapshot>,
        blocked: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let live = self.library();
        if let Some(snapshot) = running {
            let stopping = snapshot.stopping;
            let library = live.clone();
            // Only the scan needs the catalog to be stopped through, so only
            // its button goes inert when the workspace is gone.
            let inert = stopping || (library.is_none() && job == Job::Scan);
            return Some(
                settings_ui::small_button(
                    if stopping {
                        rox_i18n::t!("tasks-stopping")
                    } else {
                        rox_i18n::t!("tasks-stop")
                    },
                    icons::STOP,
                    inert,
                    cx.listener(move |_, _, _, cx| job.stop(library.as_ref(), cx)),
                )
                .into_any_element(),
            );
        }
        let (label, icon) = job.start_label()?;
        Some(
            settings_ui::small_button(
                label,
                icon,
                blocked || live.is_none(),
                cx.listener(move |this: &mut Self, _, _, cx| this.start(job, cx)),
            )
            .into_any_element(),
        )
    }

    /// The X that clears a finished dynamic row. Only there once the job
    /// has stopped: a running one has a Stop beside it, and the two are
    /// different enough that they shouldn't both be there. Standing rows
    /// never have one, since there's nothing to clear them to.
    fn dismiss(&self, job: Job, running: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        if running || job.start_label().is_some() {
            return None;
        }
        Some(
            settings_ui::icon_button(
                icons::CLOSE,
                false,
                // The standing rows returned above, so this is the only kind
                // that reaches here.
                cx.listener(move |_, _, _, cx| match job {
                    Job::LovedImport => import::dismiss(cx),
                    Job::Convert => convert::dismiss(cx),
                    Job::Bake => bake::dismiss(cx),
                    // The standing rows have no X to reach this.
                    _ => {}
                }),
            )
            .into_any_element(),
        )
    }

    /// Set a job going. The scan starts on the press, the same as the
    /// menubar's rescan button: it's minutes, it takes no settings, and
    /// nothing it does is hard to undo. The two passes go through the shared
    /// prompt instead, which is where their worker count and their estimate
    /// are set, so starting one from here is the same decision it is from the
    /// settings page rather than a shortcut around it.
    fn start(&mut self, job: Job, cx: &mut Context<Self>) {
        let Some(library) = self.library() else {
            return;
        };
        match job {
            Job::Scan => library.update(cx, |library, cx| library.rescan(cx)),
            Job::Acoustic => pass_prompt::raise(self, pass_prompt::Pass::Acoustic, library, cx),
            Job::ReplayGain => pass_prompt::raise(self, pass_prompt::Pass::ReplayGain, library, cx),
            Job::Tempo => pass_prompt::raise(self, pass_prompt::Pass::Tempo, library, cx),
            // Watched, not started: none of these has a button here to reach
            // this.
            Job::LovedImport | Job::Convert | Job::Bake => {}
        }
    }

    fn body(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let dynamic = self.dynamic(cx);
        div()
            .id("tasks")
            .size_full()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .p(tokens::SPACE_MD)
            // The standing rows fit the default frame, but a dynamic one, or
            // a resize down, shouldn't clip the bottom off the window.
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(JOBS.map(|job| self.row(job, cx)))
            // The rule says these last ones are a different kind of thing:
            // what happened, rather than what this window can set going.
            .when(!dynamic.is_empty(), |d| {
                d.child(div().flex_none().h(px(1.)).bg(palette::border()))
            })
            .children(dynamic.into_iter().map(|job| self.row(job, cx)))
            // Without a library there's nothing to drive: the workspace this
            // window opened over is gone, and the rows are reading its last
            // word rather than anything live.
            .when(self.library().is_none(), |d| {
                d.child(muted(rox_i18n::t!("tasks-no-library-window").to_string()))
            })
    }
}

impl Render for TasksWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // With no workspace player to theme to, tint to the window's own id,
        // which the palette map doesn't know, so it reads the base palette.
        let player = self.player.unwrap_or_else(|| cx.entity().entity_id());
        palette::note_focus(player, window.is_window_active(), cx);
        // Resampled here rather than on a timer; see the note in `new`.
        self.sample(cx);
        panel::window_body(player, || {
            div()
                .size_full()
                .track_focus(&self.focus)
                .flex()
                .flex_col()
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .child(self.body(cx))
                        // Fades out when idle, same as the panels.
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .child(Scrollbar::vertical(&self.scroll)),
                        ),
                )
                // The start prompt floats over the rows on its own occluding
                // layer, last so it paints on top of them.
                .children(pass_prompt::overlay(self, window, cx))
                .into_any_element()
        })
    }
}

/// The panel one job is drawn in.
fn card() -> Div {
    div()
        .flex()
        .flex_col()
        .flex_none()
        .gap(tokens::SPACE_SM)
        .p(tokens::SPACE_MD)
        .rounded(tokens::RADIUS)
        .bg(palette::bg_control())
        .border_1()
        .border_color(palette::border())
}

/// How far along, as a filled strip. The matching window's confidence bar at
/// the height a progress readout needs.
fn bar(fraction: f32) -> Div {
    div()
        .h(px(4.))
        .w_full()
        .rounded(px(2.))
        .bg(palette::bg_root())
        .child(
            div()
                .h_full()
                .rounded(px(2.))
                .w(relative(fraction.clamp(0.0, 1.0)))
                .bg(palette::accent()),
        )
}

fn muted(text: String) -> Div {
    div()
        .text_xs()
        .text_color(palette::text_muted())
        .child(SharedString::from(text))
}

fn icon(path: &'static str) -> impl IntoElement {
    gpui::svg()
        .path(path)
        .size(px(14.))
        .flex_none()
        .text_color(palette::text_muted())
}
