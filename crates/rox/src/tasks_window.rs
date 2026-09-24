//! The tasks window: the long library jobs, running or not, so the settings
//! window doesn't have to stay open to watch one.
//!
//! Six standing rows, always there: the scan, which belongs to a workspace's
//! catalog, and the app-global passes ([`crate::embeddings`],
//! [`crate::replaygain_job`], [`crate::tempo_job`],
//! [`crate::sortnames_job`], [`crate::romanize_job`]). Idle, a row says what
//! it would cost, since that's what someone opens this with before starting
//! anything.
//!
//! Dynamic rows are jobs started elsewhere (the Last.fm imports, a
//! conversion, a bake). They appear when one runs, stay for the session to
//! report what it did, and sit above the standing rows so they're never
//! pushed off the bottom.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui::{
    AnyElement, App, Bounds, Context, Div, Entity, EntityId, FocusHandle, Global, ScrollHandle,
    SharedString, Stateful, Subscription, WeakEntity, Window, WindowHandle, div, prelude::*, px,
    relative, size,
};
use gpui_component::Root;
use gpui_component::scroll::Scrollbar;

use crate::lastfm::{import, plays_import};
use crate::{
    bake, convert, embeddings, pass_prompt, replaygain_job, romanize_job, sortnames_job, tempo_job,
};
use rox_core::settings::{LayoutSize, Settings};
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_panel_api::panel;
use rox_panel_kit::ui as settings_ui;
use rox_services::catalog::{Library, LibraryEvent, ScanStatus};

/// Twice a second: this redraws every window, and a pass runs for hours.
const TICK: Duration = Duration::from_millis(500);

/// Repaint every window while a pass runs, and once more when it stops.
/// The passes are app-global with nothing to observe, so without this the
/// menubar chip would freeze at its first count. The scan doesn't need it:
/// its catalog entity notifies as it counts.
pub fn repaint_while_running(cx: &mut App) {
    crate::integrations::taskbar::watch(cx);
    if cx.try_global::<Ticking>().is_some_and(|t| t.0) {
        return;
    }
    cx.set_global(Ticking(true));
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(TICK).await;
            let live = cx.update(|cx| {
                // The falling edge repaints too, so the chip clears and the bar finishes.
                cx.refresh_windows();
                embeddings::progress(cx).is_some()
                    || replaygain_job::progress(cx).is_some()
                    || tempo_job::progress(cx).is_some()
                    || sortnames_job::progress(cx).is_some()
                    || romanize_job::progress(cx).is_some()
                    || import::progress(cx).is_some()
                    || plays_import::progress(cx).is_some()
                    || convert::progress(cx).is_some()
                    || bake::progress(cx).is_some()
            });
            if !matches!(live, Ok(true)) {
                cx.update(|cx| cx.set_global(Ticking(false))).ok();
                break;
            }
        }
    })
    .detach();
}

#[derive(Default)]
struct Ticking(bool);

impl Global for Ticking {}

/// The menubar's tasks control: a plain icon idle, a chip with the count
/// while anything runs. The scan isn't counted here; it has its own badge.
pub fn control<P: 'static>(cx: &mut Context<P>) -> Stateful<Div> {
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
    if let Some(job) = sortnames_job::progress(cx) {
        live.push((
            Job::SortNames.icon(),
            rox_i18n::t!("tasks-filling", progress = share(job.done(), job.total())).to_string(),
        ));
    }
    if let Some(job) = romanize_job::progress(cx) {
        live.push((
            Job::Romanize.icon(),
            rox_i18n::t!(
                "tasks-romanizing",
                progress = share(job.done(), job.total())
            )
            .to_string(),
        ));
    }
    if let Some(job) = import::progress(cx) {
        live.push((
            Job::LovedImport.icon(),
            rox_i18n::t!("tasks-importing", progress = share(job.done(), job.total())).to_string(),
        ));
    }
    if let Some(job) = plays_import::progress(cx) {
        live.push((
            Job::PlaysImport.icon(),
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
            icons::LIST_CHECKS,
            rox_i18n::t!("tasks-chip-count", count = several as u64).to_string(),
        )),
    };
    let open = cx.listener(|_, _, _, cx| open(cx));
    let tip = panel::Tip::keyed("tasks", rox_i18n::t!("tasks-tip"));
    let Some((path, label)) = running else {
        return tip.apply(
            div()
                .flex_none()
                .p(tokens::ICON_PAD)
                .rounded(tokens::RADIUS)
                .hover(|d| d.bg(palette::bg_control()))
                .cursor_pointer()
                .child(icon(icons::LIST_CHECKS))
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

/// The worker count is part of the answer: the same library is four hours
/// or one depending on what it may use.
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

fn share(done: usize, total: usize) -> String {
    if total == 0 {
        return "...".into();
    }
    rox_i18n::format::format_percent((done.min(total) * 100 / total).min(100) as f64)
}

const MIN: gpui::Size<gpui::Pixels> = gpui::Size {
    width: px(420.),
    height: px(320.),
};

struct OpenTasks(WindowHandle<Root>);

impl Global for OpenTasks {}

/// Deferred: callers are inside another entity's update, and reading the
/// front workspace for the tint mid-update would panic.
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
    // Held weakly, so a window that outlives its workspace goes inert rather
    // than keeping a dead library alive.
    let front = rox_panel_api::windows::front_workspace(cx).map(|(_, state)| state);
    let player = front.as_ref().map(|state| state.player.entity_id());
    let library = front.map(|state| state.library);
    let (width, height) = Settings::load()
        .windows
        .tasks
        .filter(|s| s.width >= f32::from(MIN.width) && s.height >= f32::from(MIN.height))
        .map(|s| (s.width, s.height))
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Job {
    Scan,
    Acoustic,
    ReplayGain,
    Tempo,
    SortNames,
    Romanize,
    LovedImport,
    PlaysImport,
    Convert,
    Bake,
}

/// Cheapest first, and the passes read what the scan writes. Romanization
/// sits after the sort-name fill it finishes, which should run first.
const JOBS: [Job; 6] = [
    Job::Scan,
    Job::Acoustic,
    Job::ReplayGain,
    Job::Tempo,
    Job::SortNames,
    Job::Romanize,
];

impl Job {
    fn label(self) -> SharedString {
        match self {
            Job::Scan => rox_i18n::t!("tasks-job-scan"),
            Job::Acoustic => rox_i18n::t!("tasks-job-acoustic"),
            Job::ReplayGain => rox_i18n::t!("tasks-job-replaygain"),
            Job::Tempo => rox_i18n::t!("tasks-job-tempo"),
            Job::SortNames => rox_i18n::t!("tasks-job-sortnames"),
            Job::Romanize => rox_i18n::t!("tasks-job-romanize"),
            Job::LovedImport => rox_i18n::t!("tasks-job-loved-import"),
            Job::PlaysImport => rox_i18n::t!("tasks-job-plays-import"),
            Job::Convert => rox_i18n::t!("tasks-job-convert"),
            Job::Bake => "Embed Stored Metadata".into(),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Job::Scan => icons::REFRESH_CW,
            Job::Acoustic => icons::FLASK,
            Job::ReplayGain => icons::GAUGE,
            Job::Tempo => icons::CLOCK,
            Job::SortNames => icons::ALIGN_LEFT,
            Job::Romanize => icons::GLOBE,
            Job::LovedImport => icons::HEART,
            Job::PlaysImport => icons::PLAY,
            Job::Convert => icons::AUDIO_LINES,
            Job::Bake => icons::UPLOAD,
        }
    }

    /// None for a job this window only watches.
    fn start_label(self) -> Option<(SharedString, &'static str)> {
        match self {
            Job::Scan => Some((rox_i18n::t!("tasks-start-rescan"), icons::REFRESH_CW)),
            Job::Acoustic => Some((rox_i18n::t!("tasks-start-analyze-missing"), icons::FLASK)),
            Job::ReplayGain => Some((rox_i18n::t!("tasks-start-measure-missing"), icons::GAUGE)),
            Job::Tempo => Some((rox_i18n::t!("tasks-start-analyze-missing"), icons::CLOCK)),
            Job::SortNames => Some((rox_i18n::t!("tasks-start-fill-missing"), icons::ALIGN_LEFT)),
            Job::Romanize => Some((rox_i18n::t!("tasks-start-romanize"), icons::GLOBE)),
            Job::LovedImport | Job::PlaysImport => None,
            Job::Convert => None,
            Job::Bake => None,
        }
    }

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
            Job::SortNames => sortnames_job::stop(cx),
            Job::Romanize => romanize_job::stop(cx),
            Job::LovedImport => import::stop(cx),
            Job::PlaysImport => plays_import::stop(cx),
            Job::Convert => convert::stop(cx),
            Job::Bake => bake::stop(cx),
        }
    }
}

struct Snapshot {
    done: usize,
    total: usize,
    failed: usize,
    current: String,
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

    /// Unmatched loved tracks count as failed: this library has no home for them.
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

    fn plays_import(job: &plays_import::Progress) -> Snapshot {
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

    /// Files the plan skipped never became work; the finished line reports them.
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

    fn sortnames(job: &sortnames_job::Progress) -> Snapshot {
        Snapshot {
            done: job.done(),
            total: job.total(),
            failed: job.failed(),
            current: job.current(),
            current_is_path: false,
            eta: job.eta_secs(),
            stopping: job.stopping(),
        }
    }

    fn romanize(job: &romanize_job::Progress) -> Snapshot {
        Snapshot {
            done: job.done(),
            total: job.total(),
            failed: job.failed(),
            current: job.current(),
            current_is_path: false,
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

    /// An unreadable file stays in the library, so a scan has no failed count.
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

/// (done, total) summed over the running jobs, for the taskbar button.
/// Summing rather than averaging, since every job counts files; a job with
/// no total yet adds nothing.
pub(crate) fn aggregate(cx: &mut App) -> Option<(usize, usize)> {
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
    running.extend(
        sortnames_job::progress(cx)
            .as_deref()
            .map(Snapshot::sortnames),
    );
    running.extend(
        romanize_job::progress(cx)
            .as_deref()
            .map(Snapshot::romanize),
    );
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

#[derive(Clone)]
struct Finished {
    done: usize,
    failed: usize,
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

/// None where the idle line already covers it.
struct Blocked(Option<SharedString>);

/// The scan isn't here: it's read off the catalog when a row draws.
#[derive(Default)]
struct Live {
    acoustic: Option<Arc<rox_acoustic::Progress>>,
    replaygain: Option<Arc<replaygain_job::Progress>>,
    tempo: Option<Arc<tempo_job::Progress>>,
    sortnames: Option<Arc<sortnames_job::Progress>>,
    romanize: Option<Arc<romanize_job::Progress>>,
}

/// Each field is a pass over the tracks table or a settings read, so these
/// are re-read on library events and pass ends, never per frame.
#[derive(Default)]
struct Facts {
    roots: usize,
    last_scan: i64,
    /// Per model: each model describes the library separately.
    acoustic_label: String,
    acoustic: rox_library::embeddings::Coverage,
    acoustic_on: bool,
    acoustic_estimate: Option<String>,
    rg_missing: u64,
    rg_total: u64,
    rg_estimate: Option<String>,
    bpm: rox_library::store::BpmCoverage,
    tempo_on: bool,
    tempo_estimate: Option<String>,
    /// Counted in artists, since one lookup fixes every row an artist is on.
    sort_missing: u64,
    sort_total: u64,
    /// The prompt opens on this narrower scope, so the estimate prices it.
    sort_non_latin: u64,
    sort_estimate: Option<String>,
    romanize_missing: u64,
    romanize_total: u64,
    /// Kanji only run with the Japanese dictionary installed; the rest of the
    /// pass goes ahead either way.
    romanize_kanji: u64,
    romanize_estimate: Option<String>,
}

struct TasksWindow {
    /// None themes to the base palette.
    player: Option<EntityId>,
    /// Weak: the window outlives its workspace and goes inert without it.
    library: Option<WeakEntity<Library>>,
    _subs: Vec<Subscription>,
    live: Live,
    acoustic_done: Option<Finished>,
    replaygain_done: Option<Finished>,
    tempo_done: Option<Finished>,
    sortnames_done: Option<Finished>,
    romanize_done: Option<Finished>,
    /// Kept beside `Finished` rather than in it: no other pass has one.
    romanize_skipped: usize,
    facts: Facts,
    prompt: Option<pass_prompt::Prompt>,
    value_edit: panel::ValueEdit,
    dialog_focus: FocusHandle,
    /// A window holding focus nowhere never sees a key.
    focus: FocusHandle,
    scroll: ScrollHandle,
}

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
        // The OS close button never runs remove_window, so persist the size here.
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
        // No poll of its own: [`repaint_while_running`] redraws every window while
        // a pass runs, and render resamples.
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
            sortnames_done: None,
            romanize_done: None,
            romanize_skipped: 0,
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

    fn library(&self) -> Option<Entity<Library>> {
        self.library.as_ref().and_then(|library| library.upgrade())
    }

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
        let sort = sortnames_job::coverage(library.projection().map(|p| p.as_ref()));
        let romanize = romanize_job::coverage(
            library.projection().map(|p| p.as_ref()),
            &romanize_job::stale(&library.db_path()),
        );
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
                sort_missing: sort.missing,
                sort_total: sort.total,
                sort_non_latin: sort.non_latin,
                // Priced off the rate limit: the service sets this pass's speed, so there's
                // an estimate before it has ever run.
                sort_estimate: priced(sortnames_job::PACE, sort.non_latin, 1),
                romanize_missing: romanize.missing,
                romanize_total: romanize.total,
                romanize_kanji: romanize.kanji,
                romanize_estimate: priced(settings.session.romanize_pace, romanize.missing, 1),
            };
    }

    fn sample(&mut self, cx: &mut Context<Self>) {
        let mut ended = false;
        if let Some(job) = self.live.acoustic.take()
            && embeddings::progress(cx).is_none()
        {
            self.acoustic_done = Some(Finished {
                done: job.done(),
                failed: job.failed(),
                stopped: job.stopping(),
            });
            ended = true;
        }
        if let Some(job) = self.live.replaygain.take()
            && replaygain_job::progress(cx).is_none()
        {
            self.replaygain_done = Some(Finished {
                done: job.done(),
                failed: job.failed(),
                stopped: job.stopping(),
            });
            ended = true;
        }
        if let Some(job) = self.live.tempo.take()
            && tempo_job::progress(cx).is_none()
        {
            self.tempo_done = Some(Finished {
                done: job.done(),
                failed: job.failed(),
                stopped: job.stopping(),
            });
            ended = true;
        }
        if let Some(job) = self.live.sortnames.take()
            && sortnames_job::progress(cx).is_none()
        {
            self.sortnames_done = Some(Finished {
                done: job.done(),
                failed: job.failed(),
                stopped: job.stopping(),
            });
            ended = true;
        }
        if let Some(job) = self.live.romanize.take()
            && romanize_job::progress(cx).is_none()
        {
            self.romanize_done = Some(Finished {
                done: job.done(),
                failed: job.failed(),
                stopped: job.stopping(),
            });
            self.romanize_skipped = job.skipped();
            ended = true;
        }
        self.live.acoustic = embeddings::progress(cx);
        self.live.replaygain = replaygain_job::progress(cx);
        self.live.tempo = tempo_job::progress(cx);
        self.live.sortnames = sortnames_job::progress(cx);
        self.live.romanize = romanize_job::progress(cx);
        // A restarted pass clears the last one's finished line.
        if self.live.acoustic.is_some() {
            self.acoustic_done = None;
        }
        if self.live.replaygain.is_some() {
            self.replaygain_done = None;
        }
        if self.live.tempo.is_some() {
            self.tempo_done = None;
        }
        if self.live.sortnames.is_some() {
            self.sortnames_done = None;
        }
        if self.live.romanize.is_some() {
            self.romanize_done = None;
        }
        if ended {
            self.read_facts(cx);
        }
    }

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
            Job::SortNames => self.live.sortnames.as_ref().map(|j| Snapshot::sortnames(j)),
            Job::Romanize => self.live.romanize.as_ref().map(|j| Snapshot::romanize(j)),
            // Read live rather than off the poll: the job is seconds long.
            Job::LovedImport => import::progress(cx).as_deref().map(Snapshot::import),
            Job::PlaysImport => plays_import::progress(cx)
                .as_deref()
                .map(Snapshot::plays_import),
            Job::Convert => convert::progress(cx).as_deref().map(Snapshot::convert),
            Job::Bake => bake::progress(cx).as_deref().map(Snapshot::bake),
        }
    }

    /// Started elsewhere; once run, the row stays for the session.
    fn dynamic(&self, cx: &App) -> Vec<Job> {
        let import = import::progress(cx).is_some() || import::last(cx).is_some();
        let plays_import = plays_import::progress(cx).is_some() || plays_import::last(cx).is_some();
        let convert = convert::progress(cx).is_some() || convert::last(cx).is_some();
        let bake = bake::progress(cx).is_some() || bake::last(cx).is_some();
        import
            .then_some(Job::LovedImport)
            .into_iter()
            .chain(plays_import.then_some(Job::PlaysImport))
            .chain(convert.then_some(Job::Convert))
            .chain(bake.then_some(Job::Bake))
            .collect()
    }

    fn idle_lines(&self, job: Job, cx: &App) -> Vec<String> {
        let mut lines = Vec::new();
        match job {
            Job::Scan => {
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
                    // "All of them" only holds with no refused pile.
                    lines.push(if bpm.refused > 0 {
                        rox_i18n::t!("tasks-tempo-counted", count = bpm.covered()).to_string()
                    } else {
                        rox_i18n::t!("tasks-tempo-all", count = bpm.total()).to_string()
                    });
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
                // Retrying the refused ones lives on Settings > Library.
                if self.facts.tempo_on && bpm.refused > 0 {
                    lines
                        .push(rox_i18n::t!("tasks-tempo-refused", count = bpm.refused).to_string());
                }
                if let Some(done) = &self.tempo_done {
                    lines.push(done.line());
                }
            }
            Job::SortNames => {
                if self.facts.sort_total == 0 {
                    lines.push(rox_i18n::t!("tasks-sortnames-nothing").to_string());
                } else if self.facts.sort_missing == 0 {
                    lines.push(
                        rox_i18n::t!("tasks-sortnames-all", count = self.facts.sort_total)
                            .to_string(),
                    );
                } else {
                    let mut line = rox_i18n::t!(
                        "tasks-sortnames-partial",
                        missing = self.facts.sort_missing,
                        total = self.facts.sort_total
                    )
                    .to_string();
                    if let Some(estimate) = &self.facts.sort_estimate {
                        line.push_str(&rox_i18n::t!(
                            "tasks-sortnames-non-latin",
                            count = self.facts.sort_non_latin,
                            estimate = estimate.clone()
                        ));
                    }
                    lines.push(line);
                }
                if let Some(done) = &self.sortnames_done {
                    lines.push(done.line());
                }
            }
            Job::Romanize => {
                if self.facts.romanize_total == 0 {
                    lines.push(rox_i18n::t!("tasks-romanize-nothing").to_string());
                } else if self.facts.romanize_missing == 0 {
                    lines.push(
                        rox_i18n::t!("tasks-romanize-all", count = self.facts.romanize_total)
                            .to_string(),
                    );
                } else {
                    let mut line = rox_i18n::t!(
                        "tasks-romanize-partial",
                        missing = self.facts.romanize_missing,
                        total = self.facts.romanize_total
                    )
                    .to_string();
                    if let Some(estimate) = &self.facts.romanize_estimate {
                        line.push_str(&rox_i18n::t!(
                            "tasks-reading-takes",
                            estimate = estimate.clone()
                        ));
                    }
                    lines.push(line);
                }
                if self.facts.romanize_kanji > 0
                    && self.facts.romanize_missing > 0
                    && !romanize_job::dictionary_installed()
                {
                    lines.push(
                        rox_i18n::t!("tasks-romanize-skipping", kanji = self.facts.romanize_kanji)
                            .to_string(),
                    );
                }
                if let Some(done) = &self.romanize_done {
                    lines.push(done.line());
                    if self.romanize_skipped > 0 {
                        lines.push(
                            rox_i18n::t!(
                                "tasks-romanize-skipped",
                                count = self.romanize_skipped as u64
                            )
                            .to_string(),
                        );
                    }
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
                // Only reachable for a frame, before the first progress arrives.
                None => lines.push(rox_i18n::t!("tasks-import-reading").to_string()),
            },
            Job::PlaysImport => match plays_import::last(cx) {
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
                None => lines.push(rox_i18n::t!("tasks-import-reading").to_string()),
            },
            Job::Convert => {
                match convert::last(cx) {
                    Some(summary) => lines.push(summary.line()),
                    None => lines.push(rox_i18n::t!("tasks-convert-starting").to_string()),
                }
                if let Some(reason) = convert::last_failure(cx) {
                    lines.push(reason);
                }
            }
            Job::Bake => {
                match bake::last(cx) {
                    Some(summary) => lines.push(summary.line()),
                    None => lines.push(rox_i18n::t!("tasks-bake-writing").to_string()),
                }
                if let Some(reason) = bake::last_failure(cx) {
                    lines.push(reason);
                }
            }
        }
        lines
    }

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

    /// Only a reason that will pass earns a line; an empty backlog is already
    /// what the idle line says.
    fn blocked(&self, job: Job, cx: &App) -> Option<Blocked> {
        job.start_label()?;
        let library = self.library()?;
        // A scan rewrites the rows the passes read, and the catalog runs one
        // refresh at a time.
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
                    // The pass would load a half-written model file.
                    embeddings::models::progress(cx)
                        .map(|_| Blocked(Some(rox_i18n::t!("tasks-model-downloading"))))
                }
            }
            Job::ReplayGain => (self.facts.rg_missing == 0).then_some(Blocked(None)),
            Job::Tempo => {
                (!self.facts.tempo_on || self.facts.bpm.missing == 0).then_some(Blocked(None))
            }
            Job::SortNames => (self.facts.sort_missing == 0).then_some(Blocked(None)),
            // A missing Japanese dictionary only costs the kanji values, so it never
            // blocks the button.
            Job::Romanize => (self.facts.romanize_missing == 0).then_some(Blocked(None)),
            Job::LovedImport | Job::PlaysImport | Job::Convert | Job::Bake => None,
        }
    }

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

    fn running_lines(&self, snapshot: &Snapshot) -> Vec<Div> {
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
            // Only the scan stops through the catalog.
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

    /// Only on a finished dynamic row: a running one has Stop, and standing
    /// rows have nothing to clear to.
    fn dismiss(&self, job: Job, running: bool, cx: &mut Context<Self>) -> Option<AnyElement> {
        if running || job.start_label().is_some() {
            return None;
        }
        Some(
            settings_ui::icon_button(
                icons::CLOSE,
                false,
                cx.listener(move |_, _, _, cx| match job {
                    Job::LovedImport => import::dismiss(cx),
                    Job::PlaysImport => plays_import::dismiss(cx),
                    Job::Convert => convert::dismiss(cx),
                    Job::Bake => bake::dismiss(cx),
                    _ => {}
                }),
            )
            .into_any_element(),
        )
    }

    /// The scan starts on the press. The passes go through the shared prompt,
    /// where their worker count and estimate are set, the same decision as from
    /// the settings page.
    fn start(&mut self, job: Job, cx: &mut Context<Self>) {
        let Some(library) = self.library() else {
            return;
        };
        match job {
            Job::Scan => library.update(cx, |library, cx| library.rescan(cx)),
            Job::Acoustic => pass_prompt::raise(self, pass_prompt::Pass::Acoustic, library, cx),
            Job::ReplayGain => pass_prompt::raise(self, pass_prompt::Pass::ReplayGain, library, cx),
            Job::Tempo => pass_prompt::raise(
                self,
                pass_prompt::Pass::Tempo {
                    retry_refused: false,
                },
                library,
                cx,
            ),
            Job::SortNames => pass_prompt::raise(
                self,
                pass_prompt::Pass::SortNames {
                    scope: sortnames_job::Scope::default(),
                },
                library,
                cx,
            ),
            Job::Romanize => pass_prompt::raise(self, pass_prompt::Pass::Romanize, library, cx),
            Job::LovedImport | Job::PlaysImport | Job::Convert | Job::Bake => {}
        }
    }

    fn body(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let dynamic = self.dynamic(cx);
        div()
            .id("tasks")
            .size_full()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_MD)
            .p(tokens::SPACE_MD)
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(dynamic.iter().map(|job| self.row(*job, cx)))
            .when(!dynamic.is_empty(), |d| {
                d.child(div().flex_none().h(px(1.)).bg(palette::border()))
            })
            .children(JOBS.map(|job| self.row(job, cx)))
            .when(self.library().is_none(), |d| {
                d.child(muted(rox_i18n::t!("tasks-no-library-window").to_string()))
            })
    }
}

impl Render for TasksWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // With no workspace player, tint to this window's own id, which the
        // palette map doesn't know, so it reads the base palette.
        let player = self.player.unwrap_or_else(|| cx.entity().entity_id());
        palette::note_focus(player, window.is_window_active(), cx);
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
                        .child(
                            div()
                                .absolute()
                                .inset_0()
                                .child(Scrollbar::vertical(&self.scroll)),
                        ),
                )
                .children(pass_prompt::overlay(self, window, cx))
                .into_any_element()
        })
    }
}

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
