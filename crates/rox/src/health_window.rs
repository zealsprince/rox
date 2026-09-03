//! The library health window: one OS window beside the stats page, asking
//! the other question about a library. Stats reads the listening record;
//! this reads the library itself, and answers "how well is this tagged, and
//! what should I fix next". Coverage tiles for the tag surface, counts for
//! the structural problems, and every number a door: a tile opens its
//! offending tracks in a power search window, and the fixable ones offer the
//! fix rox already has (a pass prompt, the duplicates window).
//!
//! The overview at the top is the one number the window is worth opening
//! for: the plain share of live tracks carrying all five core tags, with the
//! per-check coverage beside it. No weighting, no composite. The share is
//! [`rox_library::health`]'s, so the widget in a transport row and the ring
//! here can never disagree about what "complete" means.
//!
//! Two cost classes, refreshed differently, following ADR 11's read cadence
//! rather than inventing one: the cheap numbers (SQL aggregates and column
//! walks over the in-memory projection) are measured entering the window and
//! when the catalog changes, never per frame. The expensive ones cost disk
//! I/O per album, so they run as one background pass; a refresh while a pass
//! is out cancels it and starts a new one rather than stacking two. The pass
//! publishes each of its four answers as that answer lands, and each tile
//! shows its own stage rather than the pass's, so three tiles fill in at once
//! while the slow album-art probe counts its way through with a bar.
//!
//! Every tile carries a sentence saying what its number counts, because a
//! big number over two words is a riddle: "82" over "Album Art" could be
//! albums with art or albums without it, and the reader shouldn't have to
//! guess which way a diagnostic points. The tiles lay out in lanes sized off
//! the page's measured width rather than a fixed count, so widening the
//! window buys columns instead of whitespace; a lane stretches its tiles to
//! one height so a tile with no fix button doesn't sit short beside one that
//! has it, and a short last lane grows its tiles into the row rather than
//! leaving it half empty.
//!
//! Nothing here writes a file. Every fix door is an existing confirmed step
//! (ADR 14), and the drill-downs open a window of their own rather than
//! touching the app-wide query, so the worst a click in here can do is put a
//! second window on screen.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    div, prelude::*, px, relative, size, AnyElement, App, Bounds, Context, Div, FocusHandle,
    FontWeight, Global, ScrollHandle, SharedString, Stateful, Subscription, Task, Window,
    WindowHandle,
};
use gpui_component::scroll::Scrollbar;
use gpui_component::Root;

use rox_core::settings::Settings;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_library::duplicates::match_duplicates;
use rox_library::health::{self, Check};
use rox_library::projection::Projection;
use rox_library::{art, store};
use rox_panel_api::charts;
use rox_panel_api::panel::{self, AppState};
use rox_panel_kit::ui::{self as settings_ui, section, SECTION_GAP};
use rox_services::backdrop::WindowBackdrop;
use rox_services::catalog::LibraryEvent;

use crate::pass_prompt;
use crate::quick_play;

/// Every offending id, however many there are: the two library calls that
/// collect ids for a drill-down take a cap, and this is what asking them for
/// no cap looks like.
///
/// There used to be a real ceiling here, because the filter set held its pin
/// as a list and every following panel asked it per row, which made a pin
/// the size of the problem quadratic. The pin is a set now, so a drill-down
/// carries the whole problem for a hash lookup a row. Sized off `i64::MAX`
/// rather than `usize::MAX` because one of the two calls spends it as a SQL
/// `LIMIT`.
const DRILL_ALL: usize = i64::MAX as usize;

/// How long a refresh waits before walking the projection.
///
/// A catalog change raises `LibraryEvent::Updated` at the start of the
/// reload and again at the end, and a running scan raises one per interim
/// batch, so a single edit arrives as a burst. Walking per event spends the
/// burst measuring rows that are about to be replaced. Long enough to
/// swallow the pair, short enough that no number here looks stuck.
const SCAN_DEBOUNCE: Duration = Duration::from_millis(200);

/// The narrowest a tile is allowed to get before a lane drops a column. Sized
/// off the longest caption in the window ("N tagged, N measured, N missing")
/// so the number line stays on one row at the count widths a real library
/// produces.
const MIN_TILE_W: f32 = 260.;

/// The widest a tile's description runs before it wraps. A lane of one
/// stretches its tile across the whole page, and a sentence carried out to
/// nine hundred pixels is a line the eye loses its place returning to;
/// around sixty characters is the measure prose reads at. Tokens carry no
/// measure, so this is a plain px value.
const DESC_MAX_W: f32 = 420.;

/// A tile's corner glyph. Sized to the label under the value rather than to
/// the value itself: it's a marker for the eye scanning the grid, not a
/// second headline.
const TILE_ICON: f32 = 14.;

/// The ceiling on lanes. Past four across the descriptions turn into columns
/// of two words and the page reads as a spreadsheet, so a very wide window
/// leaves the slack at the edge instead.
const MAX_TILE_COLS: usize = 4;

/// The width the page assumes before the probe has measured one: the default
/// window minus the body padding and the scrollbar lane, which is two lanes.
/// Only ever wrong for the first frame after opening.
const ASSUMED_CONTENT_W: f32 = 640.;

/// The overview ring's size and how thick its band is. Big enough for the
/// percentage to sit inside it at the window's text size, small enough that
/// the five check rows beside it still get the width they need at the
/// minimum window size.
const RING_SIZE: f32 = 108.;
const RING_THICKNESS: f32 = 13.;

/// The check rows' name column. Fixed so the five coverage bars start at the
/// same x and read as one chart rather than five.
const CHECK_LABEL_WIDTH: f32 = 64.;

/// The floor under the check rows' count column, and the advance the column
/// is sized in. The count has to have a fixed column for the same reason the
/// name does: "Nothing missing" and "29,629 missing" are different lengths,
/// so a count that sized itself leaves every bar ending on a different x and
/// the bar's length stops reading as coverage. The width comes off the widest
/// string the library can actually produce, and the advance rounds up,
/// because a couple of spare pixels cost the meter nothing while a couple
/// short put the bars back out of line.
const CHECK_COUNT_MIN_W: f32 = 64.;
const CHECK_COUNT_CHAR_W: f32 = 6.5;

/// The open health window, if any: opening again focuses it rather than
/// stacking a second one, the stats window's move.
struct OpenHealth(WindowHandle<Root>);

impl Global for OpenHealth {}

/// Open the library health window, or bring the open one to the front. The
/// state carries the library every count is measured over, the shared query
/// the drill-downs write, and the art bake behind the page.
pub fn open(state: AppState, cx: &mut App) {
    if let Some(open) = cx.try_global::<OpenHealth>() {
        let handle = open.0;
        if handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok()
        {
            return;
        }
    }
    // The last closed window's size, sanity-floored, the stats window's
    // restore shape.
    let (width, height) = Settings::load()
        .windows
        .health
        .filter(|s| s.width >= 400. && s.height >= 300.)
        .map(|s| (s.width, s.height))
        .unwrap_or((680., 720.));
    let bounds = Bounds::centered(None, size(px(width), px(height)), cx);
    let handle = rox_panel_api::panel::open_child_window(
        cx,
        rox_i18n::t!("health-window-title"),
        bounds,
        Some(settings_ui::MIN_SIZE),
        move |window, cx| cx.new(|cx| HealthWindow::new(state, window, cx)),
    );
    cx.set_global(OpenHealth(handle));
}

/// One tile's offending rows: how many there are, and their database ids as
/// the door into a power search window over exactly those tracks.
#[derive(Clone, Default)]
struct Offenders {
    count: u64,
    ids: Vec<i64>,
}

/// How much of the library carries sort names, per table. A sort name rides
/// the value rather than the row (`SymTable::sort`), so the artist, album
/// artist and album shares are over distinct names; titles are never
/// interned, so theirs is over rows.
#[derive(Clone, Copy, Default)]
struct SortCoverage {
    artists: (u64, u64),
    album_artists: (u64, u64),
    albums: (u64, u64),
    titles: (u64, u64),
}

/// A share as a whole percent, and 100% for a table with nothing in it: an
/// empty library has no sort names missing.
fn share(with: u64, total: u64) -> f64 {
    if total == 0 {
        return 100.;
    }
    (with as f64 / total as f64 * 100.).round()
}

/// Everything the cheap pass measures, replaced whole on each refresh.
#[derive(Default)]
struct HealthData {
    /// The five core tags' coverage, and the live-row denominator every tag
    /// tile reads against. Computed by the library crate rather than here,
    /// so the overview ring, the genre and year tiles and the transport
    /// widget all count the same thing.
    complete: health::Completeness,
    /// Unrated rows. Their own scan, and deliberately not one of the five:
    /// an unrated track isn't an untagged one, and folding a taste
    /// judgement into a coverage number makes the number mean two things.
    rating: Offenders,
    sort: SortCoverage,
    /// Rows whose artist carries no sort name, the sort tile's door.
    sort_offenders: Offenders,
    gain: store::GainCoverage,
    bpm: store::BpmCoverage,
    acoustic: rox_library::embeddings::Coverage,
}

/// Everything the background pass measures, filled in a stage at a time.
/// Default is what a tile shows before its own stage has landed, which is
/// why every count starts at zero and the tiles read the stage rather than
/// these to decide whether they have an answer yet.
#[derive(Clone, Default)]
struct PassData {
    /// Albums with no cover anywhere, out of the albums there are, and the
    /// tracks they cover.
    art_albums: u64,
    albums: u64,
    art_tracks: Offenders,
    /// Duplicate identities and the tracks inside them.
    dup_groups: u64,
    dup_tracks: u64,
    /// Albums whose track numbers have holes or are missing outright.
    gap_albums: u64,
    gap_tracks: Offenders,
    /// Tracks in a container the writer has no path for, out of the local
    /// tracks the database holds.
    unwritable: Offenders,
    files: u64,
}

/// What the art probe found for one album, and the disk state it found it
/// under. The probe reads the representative file's tags and then the whole
/// cover beside it, once per album, which is the pass's entire cost on a
/// large library. The pass reruns on every library event and every pass
/// start, so without this a click on "analyze missing" reread every cover.
/// A verdict stays good while the file and its folder keep the identity
/// they had when it was reached: an embedded cover changes the file, a
/// cover.jpg dropped in changes the folder, and nothing else the probe reads
/// can move without one of those.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ArtVerdict {
    file: (i64, i64),
    folder: (i64, i64),
    missing: bool,
}

/// The art verdicts reached so far this session, keyed by representative
/// path and shared with the pass that's out. Process-wide rather than on
/// the window, so it survives a cancel and the window closing: the window
/// measures fresh on every open, and without this every open reread every
/// cover, where two stats per album answer the same question.
static ART_CACHE: Mutex<Option<HashMap<String, ArtVerdict>>> = Mutex::new(None);

/// Run `f` over the session's art cache, created on first use. Locked per
/// call rather than per stage, so a pass that's been told to stop isn't
/// holding the map against the one that replaced it.
fn with_art_cache<R>(f: impl FnOnce(&mut HashMap<String, ArtVerdict>) -> R) -> R {
    let mut guard = ART_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    f(guard.get_or_insert_with(HashMap::new))
}

/// How often the window samples a running pass. The four stages are
/// measured in seconds to minutes and only one of them counts anything, so
/// four samples a second is a smooth bar for no real cost; the tick ends
/// with the pass rather than running against an idle window.
const PASS_TICK: Duration = Duration::from_millis(250);

/// The background pass's four answers, in the order it measures them.
///
/// Album art is last because it's the only one that touches a file: it
/// reads tags off one file per album, so on a library that has never had
/// covers fetched it is the pass. Putting it last means the other three
/// land in the first second or two instead of waiting behind it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Gaps,
    Duplicates,
    Formats,
    Art,
}

/// The stages in run order; a stage's position in here is how far the pass
/// has to have got before that stage's tile has an answer.
const STAGES: [Stage; 4] = [Stage::Gaps, Stage::Duplicates, Stage::Formats, Stage::Art];

impl Stage {
    fn position(self) -> usize {
        STAGES.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// What the tile says while this stage is the one running. Only the art
    /// probe has a count worth showing: the other three are single passes
    /// over columns already in memory, with no unit to be partway through.
    fn running_caption(self, done: u64, total: u64) -> SharedString {
        match self {
            Stage::Gaps => rox_i18n::t!("health-measuring-gaps"),
            Stage::Duplicates => rox_i18n::t!("health-measuring-duplicates"),
            Stage::Formats => rox_i18n::t!("health-measuring-formats"),
            Stage::Art => {
                rox_i18n::t!("health-measuring-art", done = int(done), total = int(total),)
            }
        }
    }
}

/// A sample of a running pass, copied into the window so that a repaint
/// reads numbers and stage markers that were taken together.
///
/// The order the sample is taken in is load-bearing: the stage marker comes
/// off the pass before the revision does, and [`Pass::land`] publishes in
/// the opposite order, so a marker saying a stage has landed can never be
/// paired with data from before it did. Reading the atomics live from the
/// render instead would put a tile one frame ahead of its own number.
#[derive(Clone, Copy, Default)]
struct PassState {
    landed: usize,
    done: u64,
    total: u64,
    stopped: bool,
}

impl PassState {
    /// Nothing more will land: every stage published, or the pass stopped.
    fn finished(&self) -> bool {
        self.stopped || self.landed >= STAGES.len()
    }

    /// One stage's state, which is the whole of what its tile draws from.
    fn cell(&self, stage: Stage) -> Cell {
        let position = stage.position();
        if position < self.landed {
            Cell::Landed
        } else if position == self.landed && !self.stopped {
            Cell::Running {
                done: self.done,
                total: self.total,
            }
        } else {
            Cell::Waiting
        }
    }
}

/// What one tile knows about its own number.
enum Cell {
    /// The pass hasn't reached this stage, or gave up before it did.
    Waiting,
    /// This stage is the one running. `total` is zero for a stage with
    /// nothing to count, which draws no bar.
    Running {
        done: u64,
        total: u64,
    },
    Landed,
}

/// A running pass as the window sees it: how far along it is, and what it
/// has published so far.
///
/// Shared behind an Arc and written with atomics and one lock, the shape
/// [`crate::replaygain_job::Progress`] uses. The window samples it on a
/// timer rather than being pushed to, because a pass that published through
/// the entity would have to hold a handle across four stages and a cancel.
#[derive(Default)]
struct Pass {
    /// How many stages have published. Also the index of the stage that's
    /// running, which is what makes a tile's own state a comparison.
    landed: AtomicUsize,
    /// The running stage's counted progress, both zero when it has nothing
    /// to count.
    done: AtomicUsize,
    total: AtomicUsize,
    /// Bumped whenever `data` changes, so the window copies it once per
    /// landing rather than once per tick.
    revision: AtomicU64,
    /// Raised when the pass stops early: cancelled, or unable to open the
    /// database. Stops the window's timer, and leaves the stages that never
    /// ran reading as waiting rather than as zero.
    stopped: AtomicBool,
    data: Mutex<PassData>,
}

impl Pass {
    /// Publish one stage's result and move on to the next. The revision
    /// moves before the stage marker does, which is the half of the
    /// ordering [`PassState`] relies on.
    fn land(&self, fill: impl FnOnce(&mut PassData)) {
        fill(&mut self.data.lock().unwrap());
        self.done.store(0, Ordering::Relaxed);
        self.total.store(0, Ordering::Relaxed);
        self.revision.fetch_add(1, Ordering::Relaxed);
        self.landed.fetch_add(1, Ordering::Relaxed);
    }

    /// Where the running stage is, for the tile's bar.
    fn tick(&self, done: usize, total: usize) {
        self.done.store(done, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }

    /// Give up without publishing the rest: the tiles behind this point stay
    /// blank rather than claiming a zero the pass never measured.
    fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }

    fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    fn snapshot(&self) -> PassData {
        self.data.lock().unwrap().clone()
    }

    /// How far along the pass is, read stage marker first.
    fn state(&self) -> PassState {
        PassState {
            landed: self.landed.load(Ordering::Relaxed),
            stopped: self.stopped.load(Ordering::Relaxed),
            done: self.done.load(Ordering::Relaxed) as u64,
            total: self.total.load(Ordering::Relaxed) as u64,
        }
    }
}

/// One refresh's reading of the catalog, taken on the UI thread and handed
/// to the walk. Everything in here is either an aggregate SQL already
/// counted or a snapshot the walk holds an Arc of, so the walk never
/// touches the entity.
struct Inputs {
    gain: store::GainCoverage,
    bpm: store::BpmCoverage,
    acoustic: rox_library::embeddings::Coverage,
    projection: Option<Arc<Projection>>,
}

struct HealthWindow {
    /// The shared state: the library every count is measured over, the query
    /// the drill-downs narrow, and the art bake the backdrop paints from.
    state: AppState,
    data: HealthData,
    pass: PassData,
    /// How far the pass that's out has got, sampled beside `pass` rather
    /// than read live: the structural tiles ask this how far along they are.
    cells: PassState,
    /// Raised for the pass that's out, so a refresh mid-pass can tell it to
    /// stop between stages rather than finish work nobody will read.
    cancel: Arc<AtomicBool>,
    /// Bumped per pass; a result carrying an older number is dropped. The
    /// flag stops the work, this stops a result that was already on its way
    /// back when the flag went up.
    generation: u64,
    /// The projection walk that's out, held rather than detached: a burst of
    /// library events replaces the pending one instead of queueing a walk
    /// per event, and closing the window drops it where it stands.
    scan: Option<Task<()>>,
    /// The same guard as `generation`, for the cheap walk: a result carrying
    /// an older number never reaches the tiles.
    scan_generation: u64,
    /// The pass prompt this window raises for the three measurable tiles,
    /// and what it needs from its host.
    prompt: Option<pass_prompt::Prompt>,
    /// The page width as of the last time it crossed a lane boundary,
    /// measured by a probe in the paint rather than known up front, which is
    /// the only way an element learns its own size in gpui. Every section
    /// reads its lane count off this one number, so the columns line up down
    /// the whole page instead of each section picking its own.
    content_width: f32,
    value_edit: panel::ValueEdit,
    dialog_focus: FocusHandle,
    scroll: ScrollHandle,
    backdrop: WindowBackdrop,
    /// A rescan, a retag, or a finished pass moves every number here.
    _library_changed: Subscription,
    /// This window pumps its own frames, so the backdrop needs its own wake
    /// on a new bake.
    _backdrop_changed: Subscription,
}

/// Closing the window stops the pass that's out. The pass reads a flag and
/// holds no handle back here, so nothing else tells it its reader has gone:
/// without this, closing the window left it probing covers to nobody, and
/// reopening put a second one beside it. The walk needs no equivalent,
/// since dropping the window drops its `Task`.
impl Drop for HealthWindow {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl pass_prompt::Host for HealthWindow {
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
        self.refresh(cx);
    }
}

impl HealthWindow {
    fn new(state: AppState, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let _library_changed = cx.subscribe(
            &state.library,
            |this: &mut Self, _, event: &LibraryEvent, cx| {
                if matches!(event, LibraryEvent::Updated) {
                    this.refresh(cx);
                }
            },
        );
        let _backdrop_changed = cx.observe(&state.now_art, |_, _, cx| cx.notify());
        // The OS close button never runs remove_window, so the frame persists
        // through the should-close hook, the stats window's move.
        window.on_window_should_close(cx, move |window, _| {
            let frame = window.window_bounds().get_bounds();
            Settings::update(move |s| {
                let state = s.windows.health.get_or_insert_with(Default::default);
                state.width = frame.size.width.into();
                state.height = frame.size.height.into();
            });
            true
        });
        let mut this = HealthWindow {
            state,
            data: HealthData::default(),
            pass: PassData::default(),
            // Nothing has been measured and nothing is running, which is
            // what a stopped sample says.
            cells: PassState {
                stopped: true,
                ..Default::default()
            },
            cancel: Arc::new(AtomicBool::new(false)),
            generation: 0,
            scan: None,
            scan_generation: 0,
            prompt: None,
            content_width: ASSUMED_CONTENT_W,
            value_edit: panel::ValueEdit::default(),
            dialog_focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            backdrop: WindowBackdrop::default(),
            _library_changed,
            _backdrop_changed,
        };
        this.refresh(cx);
        this
    }

    /// Measure the cheap half and start the expensive one. Three aggregate
    /// queries on the catalog's own connection and a walk of the projection's
    /// columns; everything that would touch a file goes to [`Self::start_pass`].
    ///
    /// Cheap is relative to the art probe rather than to a frame. Both walks
    /// are O(live rows) and both collect an id per offender, so on a large
    /// library they're tens of milliseconds with a few megabytes of Vec
    /// behind them, and they used to run right here, on the UI thread, twice
    /// per edit and once per scan interim. They go to the background
    /// executor now over the same Arc the pass takes: the numbers already on
    /// screen stay up until the new ones land, the burst is waited out
    /// rather than measured through, and a result that was in flight when
    /// the library moved again is dropped by generation.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.scan_generation += 1;
        let generation = self.scan_generation;
        // The first walk is the one the window opens on. Nothing is on
        // screen for a burst to flicker yet, so it skips the wait and the
        // page fills as soon as the executor gets to it.
        let settle = (generation > 1).then_some(SCAN_DEBOUNCE);
        self.scan = Some(cx.spawn(async move |this, cx| {
            if let Some(settle) = settle {
                cx.background_executor().timer(settle).await;
            }
            let Ok(Some(inputs)) = this.update(cx, |this, cx| {
                (this.scan_generation == generation).then(|| this.inputs(cx))
            }) else {
                return;
            };
            let Inputs {
                gain,
                bpm,
                acoustic,
                projection,
            } = inputs;
            let walked = match projection.clone() {
                Some(projection) => {
                    cx.background_executor()
                        .spawn(async move {
                            let mut data = scan_projection(&projection);
                            data.complete = health::completeness(&projection, DRILL_ALL);
                            data
                        })
                        .await
                }
                None => HealthData::default(),
            };
            this.update(cx, |this, cx| {
                if this.scan_generation != generation {
                    return;
                }
                this.data = HealthData {
                    gain,
                    bpm,
                    acoustic,
                    ..walked
                };
                this.start_pass(projection, cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// What a walk needs, read off the catalog on the UI thread: the three
    /// answers SQL gives as an aggregate, and the projection snapshot the
    /// walk itself runs over.
    fn inputs(&self, cx: &Context<Self>) -> Inputs {
        let model = rox_services::acoustic::acoustic_source();
        let library = self.state.library.read(cx);
        Inputs {
            gain: library.replaygain_breakdown(),
            bpm: library.bpm_breakdown(),
            acoustic: library.acoustic_coverage(model.id()),
            projection: library.projection().cloned(),
        }
    }

    /// Hand the expensive half to the background executor: the per-album art
    /// probe, the duplicate match, the track-number sweep, and the container
    /// breakdown, all off the UI thread and all over one snapshot of the
    /// projection.
    fn start_pass(&mut self, projection: Option<Arc<Projection>>, cx: &mut Context<Self>) {
        // Whatever is out stops where it is; a new flag rides the new pass so
        // the next cancel doesn't reach back and stop this one too.
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation += 1;
        let Some(projection) = projection else {
            self.cells = PassState {
                stopped: true,
                ..Default::default()
            };
            self.pass = PassData::default();
            return;
        };
        let db_path = self.state.library.read(cx).db_path();
        let cancel = self.cancel.clone();
        let generation = self.generation;
        let pass = Arc::new(Pass::default());
        self.cells = PassState::default();
        self.pass = PassData::default();
        let worker = pass.clone();
        cx.background_executor()
            .spawn(async move { measure(&projection, &db_path, &cancel, &worker) })
            .detach();
        // Nothing observes an Arc, so the window samples it. The generation
        // guard is the only stop this loop needs: a cancel is only ever
        // raised by another start_pass, which bumps the generation before it
        // spawns the loop that replaces this one.
        cx.spawn(async move |this, cx| {
            let mut copied = 0;
            loop {
                cx.background_executor().timer(PASS_TICK).await;
                let running = this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return false;
                    }
                    // Marker first, then data: see [`PassState`].
                    let state = pass.state();
                    let revision = pass.revision();
                    if revision != copied {
                        copied = revision;
                        this.pass = pass.snapshot();
                    }
                    this.cells = state;
                    cx.notify();
                    !state.finished()
                });
                if !matches!(running, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// How many tiles a lane holds at the width the page last measured. One
    /// answer for the whole page, so the tagging, audio and files grids share
    /// a column edge.
    fn columns(&self) -> usize {
        columns_for(self.content_width)
    }

    /// A zero-size element that reports the page's laid-out width back into
    /// the window. gpui hands an element its bounds in the paint, which is
    /// after layout has run, so the count the next frame uses is the width
    /// this frame had: a live drag lags one frame and settles the moment the
    /// drag stops.
    ///
    /// The wake only fires when the width actually moves a lane's worth,
    /// never per frame, or the window would repaint itself forever off its
    /// own measurement.
    fn width_probe(&self, cx: &mut Context<Self>) -> AnyElement {
        let known = self.content_width;
        let entity = cx.entity().downgrade();
        gpui::canvas(
            |_, _, _| {},
            move |bounds: Bounds<gpui::Pixels>, _, window, _| {
                let measured = f32::from(bounds.size.width);
                if columns_for(measured) == columns_for(known) {
                    return;
                }
                let entity = entity.clone();
                window.on_next_frame(move |_, cx| {
                    entity
                        .update(cx, |this, cx| {
                            this.content_width = measured;
                            cx.notify();
                        })
                        .ok();
                });
            },
        )
        .absolute()
        .inset_0()
        .into_any_element()
    }

    /// Open the offending tracks in the power search window, named by where
    /// they came from.
    ///
    /// A window of its own rather than the app-wide query, which is what
    /// this used to write: a look at three thousand tracks missing album art
    /// isn't a change of mind about what the library view should be showing,
    /// and it shouldn't cost the user whatever they had up. The window is a
    /// singleton, so clicking through several tiles in a row walks one
    /// window through several answers.
    fn show(&mut self, door: &Door, ids: &[i64], caption: SharedString, cx: &mut Context<Self>) {
        match door {
            Door::Ids => {
                let seed = quick_play::Seed {
                    ids: ids.to_vec(),
                    label: caption,
                };
                crate::search_window::open_seeded(self.state.clone(), seed, cx);
            }
            // The two checks the query language can say by itself go through
            // it: `-genre` covers every offending row and stays true as the
            // library changes, where an id pin is only as complete as the
            // list behind it was the moment the scan ran.
            Door::Field(field) => {
                crate::search_window::open_with_query(self.state.clone(), &format!("-{field}"), cx)
            }
        }
    }

    /// Raise the start prompt for one of the three passes, the same dialog
    /// the settings and tasks windows raise.
    fn start_pass_prompt(&mut self, pass: pass_prompt::Pass, cx: &mut Context<Self>) {
        let library = self.state.library.clone();
        pass_prompt::raise(self, pass, library, cx);
    }

    /// The headline: one ring for the share of the library carrying all
    /// five core tags, and beside it a coverage bar per check.
    ///
    /// The ring is complete against incomplete and nothing else. A slice per
    /// check would double-count every track missing two of them, so the
    /// slices would add up past the library and the picture would flatter or
    /// damn it depending on which way the overlaps fell. The per-check
    /// breakdown is the rows, where overlapping is fine because nothing is
    /// being summed.
    fn overview_section(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let health = &self.data.complete;
        let share = health.share();
        let dial = div()
            .flex_none()
            .flex()
            .flex_col()
            .items_center()
            .gap(tokens::SPACE_XS)
            .child(
                div()
                    .relative()
                    .child(charts::ring(
                        share,
                        px(RING_SIZE),
                        px(RING_THICKNESS),
                        palette::bg_control_active(),
                        palette::accent(),
                    ))
                    // The number lives over the hole rather than in the
                    // paint closure: text inside a canvas needs the text
                    // system wired through, and a centred div is free.
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_xl()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(palette::text_bright())
                            .child(SharedString::from(rox_i18n::format::format_percent(
                                (share as f64 * 100.).round(),
                            ))),
                    ),
            )
            .child(
                div()
                    .w(px(RING_SIZE))
                    .text_xs()
                    .text_center()
                    .text_color(palette::text_muted())
                    .child(rox_i18n::t!(
                        "health-overview-complete",
                        complete = int(health.complete()),
                        total = tracks_worded(health.tracks),
                    )),
            );
        // One width for all five rows, off the library's own total: the
        // widest count any of them can say is every track missing it.
        let count_w = px(count_column_w(self.data.complete.tracks));
        let rows = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(tokens::SPACE_SM)
            .children(Check::ALL.map(|check| self.check_row(check, count_w, cx)));
        section(
            rox_i18n::t!("health-section-overview"),
            None,
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_MD)
                .child(dial)
                .child(rows),
        )
    }

    /// One check's row: its name, how much of the library carries it, and
    /// what's left. The whole row is the door, the way a tile's button is,
    /// since a row with a count already reads as something to act on.
    ///
    /// The count column's width is handed in rather than measured here, so
    /// the five rows share one and the meters between them all end on the
    /// same x.
    fn check_row(&self, check: Check, count_w: gpui::Pixels, cx: &mut Context<Self>) -> AnyElement {
        let missing = self.data.complete.missing(check);
        let count = missing.count;
        let door = check_door(check);
        div()
            .id(SharedString::from(format!(
                "health-check-{}",
                check_key(check)
            )))
            .flex()
            .flex_row()
            .items_center()
            .gap(tokens::SPACE_SM)
            .when(count > 0, |d| {
                // The ids are only cloned for a row that has somewhere to
                // go; a fully tagged library repaints without copying five
                // empty lists a frame.
                let ids = missing.ids.clone();
                let caption = seed_caption(check_label(check), count);
                d.cursor_pointer()
                    .on_click(cx.listener(move |this: &mut Self, _, _, cx| {
                        this.show(&door, &ids, caption.clone(), cx)
                    }))
            })
            .child(
                div()
                    .w(px(CHECK_LABEL_WIDTH))
                    .flex_none()
                    .truncate()
                    .text_xs()
                    .child(check_label(check)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(meter(self.data.complete.coverage(check), px(6.))),
            )
            .child(
                div()
                    .w(count_w)
                    .flex_none()
                    .text_right()
                    .text_xs()
                    .text_color(if count == 0 {
                        palette::text_faint()
                    } else {
                        palette::text_muted()
                    })
                    .child(if count == 0 {
                        rox_i18n::t!("health-complete")
                    } else {
                        rox_i18n::t!("health-overview-missing", missing = int(count))
                    }),
            )
            .into_any_element()
    }

    /// The tag surface: what the files say about themselves, and the sort
    /// names that decide how the library buckets.
    fn tagging_section(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let genre = self.data.complete.missing(Check::Genre);
        let year = self.data.complete.missing(Check::Year);
        let tiles: Vec<AnyElement> = vec![
            self.genre_tile(genre, cx),
            self.missing_tile(
                icons::CALENDAR,
                rox_i18n::t!("health-tile-year"),
                rox_i18n::t!("health-desc-year"),
                year.count,
                &year.ids,
                Some(Door::Field("year")),
                cx,
            ),
            self.missing_tile(
                icons::STAR,
                rox_i18n::t!("health-tile-rating"),
                rox_i18n::t!("health-desc-rating"),
                self.data.rating.count,
                &self.data.rating.ids,
                Some(Door::Ids),
                cx,
            ),
            self.sort_tile(cx),
        ];
        section(
            rox_i18n::t!("health-section-tags"),
            None,
            grid(tiles, self.columns()),
        )
    }

    /// The three numbers a pass fills in, each with the prompt that starts it.
    fn audio_section(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let gain = self.data.gain;
        let bpm = self.data.bpm;
        let acoustic = self.data.acoustic;
        let mut tiles: Vec<AnyElement> = Vec::new();
        tiles.push(
            tile(
                icons::GAUGE,
                rox_i18n::t!("health-tile-replaygain"),
                count_value(gain.missing),
                rox_i18n::t!("health-desc-replaygain"),
                rox_i18n::t!(
                    "health-caption-split",
                    tagged = int(gain.tagged),
                    measured = int(gain.measured),
                    missing = int(gain.missing),
                ),
                None,
                self.pass_button(
                    "health-rg",
                    rox_i18n::t!("health-fix-measure"),
                    pass_prompt::Pass::ReplayGain,
                    crate::replaygain_job::progress(cx).is_some(),
                    gain.missing == 0,
                    cx,
                ),
            )
            .into_any_element(),
        );
        // The tile counts what Analyze Missing would work through, so it
        // stays on `missing`. The refused pile joins the caption instead:
        // it's the rest of the library's untimed tracks, and a Tempo tile
        // reading 0 with nine thousand of them about would be a lie by
        // omission. Retrying them is the Library page's button, not this
        // window's.
        let bpm_caption = if bpm.refused > 0 {
            rox_i18n::t!(
                "health-caption-split-refused",
                tagged = int(bpm.tagged),
                measured = int(bpm.measured),
                missing = int(bpm.missing),
                refused = int(bpm.refused),
            )
        } else {
            rox_i18n::t!(
                "health-caption-split",
                tagged = int(bpm.tagged),
                measured = int(bpm.measured),
                missing = int(bpm.missing),
            )
        };
        tiles.push(
            tile(
                icons::ACTIVITY,
                rox_i18n::t!("health-tile-tempo"),
                count_value(bpm.missing),
                rox_i18n::t!("health-desc-tempo"),
                bpm_caption,
                None,
                self.pass_button(
                    "health-tempo",
                    rox_i18n::t!("health-fix-analyze"),
                    pass_prompt::Pass::Tempo {
                        retry_refused: false,
                    },
                    crate::tempo_job::progress(cx).is_some(),
                    bpm.missing == 0,
                    cx,
                ),
            )
            .into_any_element(),
        );
        let missing = acoustic.missing() as u64;
        tiles.push(
            tile(
                icons::AUDIO_WAVEFORM,
                rox_i18n::t!("health-tile-acoustic"),
                count_value(missing),
                rox_i18n::t!("health-desc-acoustic"),
                rox_i18n::t!(
                    "health-caption-missing",
                    missing = int(missing),
                    total = tracks_worded(acoustic.total as u64),
                ),
                None,
                self.pass_button(
                    "health-acoustic",
                    rox_i18n::t!("health-fix-analyze"),
                    pass_prompt::Pass::Acoustic,
                    crate::embeddings::progress(cx).is_some(),
                    missing == 0,
                    cx,
                ),
            )
            .into_any_element(),
        );
        section(
            rox_i18n::t!("health-section-audio"),
            None,
            grid(tiles, self.columns()),
        )
    }

    /// What the background pass found: the shape of the collection rather
    /// than the tags on it.
    fn files_section(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let tiles: Vec<AnyElement> = vec![
            self.art_tile(cx),
            self.duplicates_tile(cx),
            self.gaps_tile(cx),
            self.formats_tile(cx),
        ];
        section(
            rox_i18n::t!("health-section-files"),
            None,
            grid(tiles, self.columns()),
        )
    }

    /// Albums with no cover in their tags and none beside them on disk.
    fn art_tile(&self, cx: &mut Context<Self>) -> AnyElement {
        let (value, caption, bar) = self.pass_cell(
            Stage::Art,
            self.pass.art_albums,
            rox_i18n::t!(
                "health-caption-art",
                albums = int(self.pass.art_albums),
                total = albums_worded(self.pass.albums),
                tracks = tracks_worded(self.pass.art_tracks.count),
            ),
        );
        tile(
            icons::IMAGE,
            rox_i18n::t!("health-tile-art"),
            value,
            rox_i18n::t!("health-desc-art"),
            caption,
            bar,
            self.drill_button(
                "health-art",
                rox_i18n::t!("health-tile-art"),
                self.pass.art_tracks.count,
                &self.pass.art_tracks.ids,
                Door::Ids,
                cx,
            ),
        )
        .into_any_element()
    }

    /// Tracks whose files carry no genre. The only tag tile with two
    /// doors: the drill-down every other one has, and the tagger, which is
    /// the fix rather than a look at the problem. Built here rather than
    /// through [`Self::missing_tile`] because that helper takes exactly one
    /// door, and widening it for the one tile that wants two would put an
    /// Option nobody else fills through every caller.
    fn genre_tile(&self, genre: &health::Missing, cx: &mut Context<Self>) -> AnyElement {
        let count = genre.count;
        let caption = if count == 0 {
            rox_i18n::t!("health-complete")
        } else {
            rox_i18n::t!(
                "health-caption-missing",
                missing = int(count),
                total = tracks_worded(self.data.complete.tracks),
            )
        };
        let drill = self.drill_button(
            "health-genre",
            rox_i18n::t!("health-tile-genre"),
            count,
            &genre.ids,
            Door::Field("genre"),
            cx,
        );
        // Nothing to tag means nothing to open the tagger for; the tile
        // reads as complete and offers no door at all.
        let fix = (count > 0).then(|| {
            let state = self.state.clone();
            settings_ui::small_button(
                rox_i18n::t!("health-fix-genres"),
                icons::TAG,
                false,
                cx.listener(move |_, _, _, cx| crate::genre_tagger::open(state.clone(), cx)),
            )
            .keyed("health-tag-genres")
            .into_any_element()
        });
        let action = (drill.is_some() || fix.is_some()).then(|| {
            div()
                .flex()
                .flex_row()
                .gap(tokens::SPACE_XS)
                .children(drill)
                .children(fix)
                .into_any_element()
        });
        tile(
            icons::TAG,
            rox_i18n::t!("health-tile-genre"),
            count_value(count),
            rox_i18n::t!("health-desc-genre"),
            caption,
            None,
            action,
        )
        .into_any_element()
    }

    /// Tag identities the library holds more than once, with the window that
    /// picks which copy to keep.
    fn duplicates_tile(&self, cx: &mut Context<Self>) -> AnyElement {
        let (value, caption, bar) = self.pass_cell(
            Stage::Duplicates,
            self.pass.dup_groups,
            rox_i18n::t!(
                "health-caption-duplicates",
                groups = groups_worded(self.pass.dup_groups),
                tracks = tracks_worded(self.pass.dup_tracks),
            ),
        );
        // Zero until the stage lands, so the button appears with the number
        // rather than needing its own check on the stage.
        let button = (self.pass.dup_groups > 0).then(|| {
            let state = self.state.clone();
            settings_ui::small_button(
                rox_i18n::t!("health-fix-duplicates"),
                icons::COPY,
                false,
                cx.listener(move |_, _, _, cx| {
                    crate::duplicates::open(
                        state.library.clone(),
                        state.thumbs.clone(),
                        state.now_art.clone(),
                        cx,
                    );
                }),
            )
            .keyed("health-duplicates")
            .into_any_element()
        });
        tile(
            icons::COPY,
            rox_i18n::t!("health-tile-duplicates"),
            value,
            rox_i18n::t!("health-desc-duplicates"),
            caption,
            bar,
            button,
        )
        .into_any_element()
    }

    /// Albums whose track numbers have a hole under their highest, or whose
    /// tracks carry no number at all.
    fn gaps_tile(&self, cx: &mut Context<Self>) -> AnyElement {
        let (value, caption, bar) = self.pass_cell(
            Stage::Gaps,
            self.pass.gap_albums,
            rox_i18n::t!(
                "health-caption-gaps",
                albums = int(self.pass.gap_albums),
                total = albums_worded(self.pass.albums),
            ),
        );
        tile(
            icons::HASH,
            rox_i18n::t!("health-tile-gaps"),
            value,
            rox_i18n::t!("health-desc-gaps"),
            caption,
            bar,
            self.drill_button(
                "health-gaps",
                rox_i18n::t!("health-tile-gaps"),
                self.pass.gap_tracks.count,
                &self.pass.gap_tracks.ids,
                Door::Ids,
                cx,
            ),
        )
        .into_any_element()
    }

    /// Tracks rox has no tag writer for, by container.
    ///
    /// The count comes off the filename extension mapped through
    /// [`store::WRITABLE_EXTENSIONS`], which mirrors `writer::file_type`;
    /// `writer::readable` and `writer::supported` are the real source of
    /// truth for "can retag", and they answer per file rather than per
    /// extension. The gap is deliberate and worth knowing about: a
    /// fragmented MP4 is refused at write time even though `.m4a` counts as
    /// writable here, and catching those would mean opening every file in the
    /// library, a scan-shaped cost for a diagnostic. So this tile reads as
    /// "formats rox has a writer for", not "files rox will definitely
    /// retag", and it undercounts on a library full of DASH-assembled m4a.
    fn formats_tile(&self, cx: &mut Context<Self>) -> AnyElement {
        let (value, caption, bar) = self.pass_cell(
            Stage::Formats,
            self.pass.unwritable.count,
            rox_i18n::t!(
                "health-caption-formats",
                unwritable = int(self.pass.unwritable.count),
                total = tracks_worded(self.pass.files),
            ),
        );
        tile(
            icons::FILE_TEXT,
            rox_i18n::t!("health-tile-writable"),
            value,
            rox_i18n::t!("health-desc-writable"),
            caption,
            bar,
            self.drill_button(
                "health-formats",
                rox_i18n::t!("health-tile-writable"),
                self.pass.unwritable.count,
                &self.pass.unwritable.ids,
                Door::Ids,
                cx,
            ),
        )
        .into_any_element()
    }

    /// The sort-name tile: the share each table carries, the door that
    /// fills the rest from MusicBrainz, and the door into the rows whose
    /// artist still has none. This is the tile a CJK-heavy library opens
    /// the window for, since it says exactly how much of it will bucket
    /// the way its owner reads it.
    fn sort_tile(&self, cx: &mut Context<Self>) -> AnyElement {
        let sort = self.data.sort;
        // The only tile with two doors, because it's the only one whose
        // problem has both a fix and a list worth reading. Fill first:
        // it's what the tile is for, and the drill is the second thought
        // of someone who wants to see which rows are behind the number.
        let fill = self.pass_button(
            "health-sort-fill",
            rox_i18n::t!("health-fix-fill"),
            pass_prompt::Pass::SortNames {
                scope: crate::sortnames_job::Scope::default(),
            },
            crate::sortnames_job::progress(cx).is_some(),
            sort.artists.0 >= sort.artists.1,
            cx,
        );
        let drill = self.drill_button(
            "health-sort",
            rox_i18n::t!("health-tile-sort-names"),
            self.data.sort_offenders.count,
            &self.data.sort_offenders.ids,
            Door::Ids,
            cx,
        );
        // Nothing to fill and nothing to show is no row at all, rather
        // than an empty one holding the tile's footer open.
        let doors = (fill.is_some() || drill.is_some()).then(|| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(tokens::SPACE_XS)
                .children(fill)
                .children(drill)
                .into_any_element()
        });
        tile(
            icons::ALIGN_LEFT,
            rox_i18n::t!("health-tile-sort-names"),
            SharedString::from(rox_i18n::format::format_percent(share(
                sort.artists.0,
                sort.artists.1,
            ))),
            rox_i18n::t!("health-desc-sort-names"),
            rox_i18n::t!(
                "health-caption-sort",
                album_artists = pct(sort.album_artists),
                albums = pct(sort.albums),
                titles = pct(sort.titles),
            ),
            None,
            doors,
        )
        .into_any_element()
    }

    /// A coverage tile over one column: the missing count large, the share it
    /// stands against underneath, and the door into the offenders.
    #[allow(clippy::too_many_arguments)]
    fn missing_tile(
        &self,
        icon: &'static str,
        label: SharedString,
        description: SharedString,
        count: u64,
        ids: &[i64],
        door: Option<Door>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let caption = if count == 0 {
            rox_i18n::t!("health-complete")
        } else {
            rox_i18n::t!(
                "health-caption-missing",
                missing = int(count),
                total = tracks_worded(self.data.complete.tracks),
            )
        };
        let key: SharedString = label.clone();
        let button =
            door.and_then(|door| self.drill_button(key, label.clone(), count, ids, door, cx));
        tile(
            icon,
            label,
            count_value(count),
            description,
            caption,
            None,
            button,
        )
        .into_any_element()
    }

    /// The "show these" control, or nothing when there's nothing to show.
    /// The title is the tile's own name, which becomes the caption over the
    /// window that opens: a filter somebody else chose is invisible
    /// otherwise, since the rows are simply fewer than the library has.
    fn drill_button(
        &self,
        key: impl Into<gpui::ElementId>,
        title: SharedString,
        count: u64,
        ids: &[i64],
        door: Door,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if count == 0 {
            return None;
        }
        let ids = ids.to_vec();
        let caption = seed_caption(title, count);
        Some(
            settings_ui::small_button(
                rox_i18n::t!("health-drill"),
                icons::FUNNEL,
                false,
                cx.listener(move |this: &mut Self, _, _, cx| {
                    this.show(&door, &ids, caption.clone(), cx)
                }),
            )
            .keyed(key)
            .into_any_element(),
        )
    }

    /// One pass's button: the prompt that starts it, gone while the pass is
    /// running or when there's nothing left for it to do.
    fn pass_button(
        &self,
        key: &'static str,
        label: SharedString,
        pass: pass_prompt::Pass,
        running: bool,
        done: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if done {
            return None;
        }
        if running {
            return Some(
                div()
                    .text_xs()
                    .text_color(palette::accent())
                    .child(rox_i18n::t!("health-running"))
                    .into_any_element(),
            );
        }
        Some(
            settings_ui::small_button(
                label,
                icons::PLAY,
                false,
                cx.listener(move |this: &mut Self, _, _, cx| this.start_pass_prompt(pass, cx)),
            )
            .keyed(key)
            .into_any_element(),
        )
    }

    /// A structural tile's number, caption and bar, off its own stage.
    ///
    /// A tile whose stage hasn't run says so and shows a dash: a zero from a
    /// pass that never got there would read as good news it hasn't earned.
    /// The running stage says what it's doing, and the one stage with a unit
    /// to count carries a bar.
    fn pass_cell(
        &self,
        stage: Stage,
        count: u64,
        landed: SharedString,
    ) -> (SharedString, SharedString, Option<f32>) {
        match self.cells.cell(stage) {
            Cell::Landed => (count_value(count), landed, None),
            Cell::Waiting => ("-".into(), rox_i18n::t!("health-waiting"), None),
            Cell::Running { done, total } => (
                "-".into(),
                stage.running_caption(done, total),
                (total > 0).then(|| done as f32 / total as f32),
            ),
        }
    }
}

/// Which door a tile's button opens: an explicit id pin, or an absence term
/// the search box shows and the user can edit.
#[derive(Clone)]
enum Door {
    Ids,
    Field(&'static str),
}

/// A count as the tile's headline number.
fn count_value(count: u64) -> SharedString {
    SharedString::from(rox_i18n::format::format_int(count as i64))
}

/// A count for a message argument.
fn int(count: u64) -> String {
    rox_i18n::format::format_int(count as i64)
}

/// A share as a percent string, for the sort tile's caption.
fn pct(share_of: (u64, u64)) -> String {
    rox_i18n::format::format_percent(share(share_of.0, share_of.1))
}

/// A track count worded by the shared plural message, so a caption that
/// embeds it never has to select on a number itself.
fn tracks_worded(count: u64) -> String {
    rox_i18n::t!("status-count-tracks", count = count).to_string()
}

/// The same for duplicate groups, whose plural is this window's own.
fn groups_worded(count: u64) -> String {
    rox_i18n::t!("health-count-groups", count = count).to_string()
}

/// What a drill-down's window says it's showing: the name of the tile or row
/// that opened it, and how many tracks came with it. One message so a
/// translator can reorder the two halves.
fn seed_caption(source: SharedString, count: u64) -> SharedString {
    rox_i18n::t!(
        "search-seed-caption",
        source = source.to_string(),
        count = tracks_worded(count),
    )
}

/// The same for albums.
fn albums_worded(count: u64) -> String {
    rox_i18n::t!("status-count-albums", count = count).to_string()
}

/// One tile: an icon and the number large over its name, a sentence saying
/// what the number counts, the caption under that, and whatever door it
/// offers on the bottom edge. Shaped like the stats window's cards, with room
/// under them for a control.
///
/// The icon sits on the value's row, muted, so a reader scanning the page
/// picks a tile out by shape before reading a word of it: the four sections
/// hold twelve numbers between them, and twelve identical boxes are twelve
/// things to read in order.
///
/// The description is the reason this window is readable by someone who
/// didn't build it: "82" over "Album Art" is a number and a noun, and every
/// reading of it is a guess until the sentence says which 82 things those
/// are. It wraps rather than truncating, because a half sentence explains
/// nothing.
///
/// Content and action are two children with `justify_between` rather than one
/// run of lines, so when the lane stretches this tile to its neighbour's
/// height the slack lands between the caption and the button instead of under
/// everything, and the buttons across a lane line up on the bottom edge.
fn tile(
    icon: &'static str,
    label: SharedString,
    value: SharedString,
    description: SharedString,
    caption: SharedString,
    bar: Option<f32>,
    action: Option<AnyElement>,
) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .justify_between()
        .p(tokens::SPACE_SM)
        .rounded(tokens::RADIUS)
        .bg(palette::bg_control())
        .border_1()
        .border_color(palette::border())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(tokens::SPACE_XS)
                        .child(
                            gpui::svg()
                                .path(icon)
                                .size(px(TILE_ICON))
                                .flex_none()
                                .text_color(palette::text_muted()),
                        )
                        .child(
                            div()
                                .text_xl()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(palette::text_bright())
                                .child(value),
                        ),
                )
                .child(div().truncate().text_xs().child(label))
                .child(
                    div()
                        .max_w(px(DESC_MAX_W))
                        .text_xs()
                        .text_color(palette::text_muted())
                        .child(description),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(palette::text_faint())
                        .child(caption),
                )
                .when_some(bar, |d, fraction| {
                    d.child(div().pt(tokens::SPACE_XS).child(meter(fraction, px(3.))))
                }),
        )
        .when_some(action, |d, action| {
            d.child(
                div()
                    .flex()
                    .flex_row()
                    .pt(tokens::SPACE_XS)
                    .child(action)
                    .into_any_element(),
            )
        })
}

/// How many tiles fit across a page of this width, gaps included: a lane of
/// `n` needs `n` tiles at [`MIN_TILE_W`] plus the `n - 1` gaps between them.
///
/// One, two or four, never three. Three lanes read as a mistake on a page
/// whose sections hold four tiles each: the last lane is always a lone tile
/// under a row of three, and the eye reads the ragged edge as broken layout
/// rather than as a count that happens to divide badly. So the page steps
/// straight from two to four when four fit, and sits at two the whole way
/// between.
///
/// Pure so the breakpoints are testable, since the thing this is easy to get
/// wrong about is the off-by-one at the edge of a lane rather than anything
/// you would see by looking at it. A width that fits nothing still gets one
/// column: a tile squeezed under its minimum still beats no page at all. A
/// NaN width fails every comparison and falls out at one rather than
/// panicking.
fn columns_for(width: f32) -> usize {
    let gap = f32::from(tokens::SPACE_SM);
    let fits = |lanes: usize| width + gap >= (MIN_TILE_W + gap) * lanes as f32;
    if fits(MAX_TILE_COLS) {
        MAX_TILE_COLS
    } else if fits(2) {
        2
    } else {
        1
    }
}

/// The width the check rows' count column claims: the wider of the two
/// things a row can say, in the current locale, at the largest count this
/// library can produce.
///
/// Estimated off character count rather than laid out by the text system.
/// The real advance is only available inside a paint, and reserving a column
/// a few pixels wide of the string costs nothing here, while asking the text
/// system for it would mean threading a `Window` through every section for a
/// number that changes when the library does.
fn count_column_w(total: u64) -> f32 {
    let widest = text_units(&rox_i18n::t!("health-complete")).max(text_units(&rox_i18n::t!(
        "health-overview-missing",
        missing = int(total)
    )));
    (widest * CHECK_COUNT_CHAR_W).ceil().max(CHECK_COUNT_MIN_W)
}

/// How many character widths a string takes, counting a CJK glyph as two.
/// Square glyphs are the difference between a column that fits the Japanese
/// string and one that clips it, and they're the only class wide enough to
/// matter at this precision.
fn text_units(text: &str) -> f32 {
    text.chars()
        .map(|c| if c >= '\u{2e80}' { 2. } else { 1. })
        .sum()
}

/// The tiles in lanes of `columns`. Every tile is `flex_1` and every lane
/// carries the same gap, so a lane spans the page whatever it holds: three
/// tiles under a lane of four come out three wide ones, a lone tile comes
/// out full width, and the left edges line up by construction because every
/// lane starts at the same x with the same first basis.
///
/// No invisible fillers hold a short lane's spare columns open. They leave
/// the row looking half empty, and because a filler and a tile don't share a
/// flex basis, the lone tile beside them comes out a few pixels narrower
/// than the tile directly above it. Growing the tiles answers both.
///
/// The lane sets no `align_items`, which leaves taffy's flex default of
/// stretch, so every tile in a lane takes the height of the tallest one. That
/// is the whole fix for a tile without an action button sitting short beside
/// one that has it: the box matches, and the button hangs off the bottom edge
/// because the tile justifies its content apart.
fn grid(tiles: Vec<AnyElement>, columns: usize) -> Div {
    let mut grid = div().flex().flex_col().gap(tokens::SPACE_SM);
    let mut tiles = tiles.into_iter().peekable();
    while tiles.peek().is_some() {
        grid = grid.child(
            div()
                .flex()
                .flex_row()
                .gap(tokens::SPACE_SM)
                .children(tiles.by_ref().take(columns)),
        );
    }
    grid
}

/// The cheap half this window measures for itself: the rating column and
/// the sort tables. The five core tags are [`health::completeness`]'s walk,
/// which the refresh runs beside this one; two column walks over an
/// in-memory projection is a rounding error next to one shared definition of
/// complete.
///
/// Sequential rather than split across cores, since it's a byte or two a row
/// against the per-file work the background pass does; tombstoned rows are
/// skipped, or every count would include rows the library has already let
/// go.
fn scan_projection(projection: &Projection) -> HealthData {
    let artist_unsorted: Vec<bool> = (0..projection.artists.strings.len())
        .map(|sym| projection.artists.sort_name(sym).is_empty())
        .collect();
    let mut data = HealthData::default();
    let mut tracks = 0u64;
    let mut titles_with = 0u64;
    for row in 0..projection.len() {
        if projection.is_dead(row as u32) {
            continue;
        }
        tracks += 1;
        let id = projection.db_id[row];
        if projection.rating[row].load(Ordering::Relaxed) == 0 {
            data.rating.count += 1;
            data.rating.ids.push(id);
        }
        if !projection.title_sort(row).is_empty() {
            titles_with += 1;
        }
        if artist_unsorted[projection.artist[row] as usize] {
            data.sort_offenders.count += 1;
            data.sort_offenders.ids.push(id);
        }
    }
    data.sort = SortCoverage {
        artists: sorted_share(&projection.artists),
        album_artists: sorted_share(&projection.album_artists),
        albums: sorted_share(&projection.albums),
        titles: (titles_with, tracks),
    };
    data
}

/// How many of a table's values carry a sort name, out of the values that
/// could. The empty value is left out of both halves: a nameless artist has
/// no sort name and never will, and counting it would make a clean library
/// look short.
fn sorted_share(table: &rox_library::projection::SymTable) -> (u64, u64) {
    let mut with = 0;
    let mut total = 0;
    for sym in 0..table.strings.len() {
        if table.strings[sym].is_empty() {
            continue;
        }
        total += 1;
        if !table.sort_name(sym).is_empty() {
            with += 1;
        }
    }
    (with, total)
}

/// A filled bar over a track, the shape both the tile progress and the
/// check rows want. Palette roles rather than a colour: the fill is the
/// accent because it's the number the eye should land on, and the track is
/// the raised control surface so an empty bar still reads as a slot.
fn meter(fraction: f32, height: gpui::Pixels) -> Div {
    div()
        .h(height)
        .w_full()
        .rounded(height)
        .bg(palette::bg_control_active())
        .child(
            div()
                .h_full()
                .w(relative(fraction.clamp(0., 1.)))
                .rounded(height)
                .bg(palette::accent()),
        )
}

/// A check's name for the overview row.
fn check_label(check: Check) -> SharedString {
    match check {
        Check::Title => rox_i18n::t!("health-tile-title"),
        Check::Artist => rox_i18n::t!("health-tile-artist"),
        Check::Album => rox_i18n::t!("health-tile-album"),
        Check::Genre => rox_i18n::t!("health-tile-genre"),
        Check::Year => rox_i18n::t!("health-tile-year"),
    }
}

/// A stable element id fragment per check, so the rows keep their identity
/// across repaints.
fn check_key(check: Check) -> &'static str {
    match check {
        Check::Title => "title",
        Check::Artist => "artist",
        Check::Album => "album",
        Check::Genre => "genre",
        Check::Year => "year",
    }
}

/// Where a check's row sends the user. Genre and year are absences the query
/// language spells, so they take the field door: it lands in the search box
/// as `-genre`, which the user can read, edit and widen, where an id pin
/// reads as nothing at all. The other three have no such term, so they pin
/// their ids.
fn check_door(check: Check) -> Door {
    match check {
        Check::Genre => Door::Field("genre"),
        Check::Year => Door::Field("year"),
        _ => Door::Ids,
    }
}

/// Whether a row carries an album name at all.
///
/// Every album-less row in the library shares one symbol, the empty one, so
/// anything keyed on the album column folds all of them into a single
/// bucket unless it asks this first. The genre tagger's album switch
/// carries the same guard for the same reason.
fn has_album(projection: &Projection, row: usize) -> bool {
    !projection.albums.strings[projection.album[row] as usize].is_empty()
}

/// The library's albums, keyed the way the art probe wants them: one entry
/// per (folder, album), holding a representative row and how many tracks it
/// covers. Folder rather than album artist, because art sits beside the
/// files: two albums of the same name in two folders each want their own
/// cover, and one folder holding a split release still has one.
///
/// Rows with no album name stay out. A folder of loose singles isn't an
/// album, and keying it on the empty symbol made it one: whichever file the
/// map seated first decided the cover verdict for every track in the
/// folder, which is one file's answer wearing a hundred files' weight. The
/// honest alternative is a unit per loose track, and that turns the one
/// stage that reads files from a read per album into a read per track,
/// which is exactly the cost class this window keeps out of the pass. So
/// the art tile counts albums the library actually names, the same rows the
/// gap check judges, and says nothing about loose files.
fn group_albums(projection: &Projection) -> HashMap<(u32, u32), (u32, u64)> {
    let mut albums: HashMap<(u32, u32), (u32, u64)> = HashMap::new();
    for row in 0..projection.len() {
        if projection.is_dead(row as u32) || !has_album(projection, row) {
            continue;
        }
        let entry = albums
            .entry((projection.folder[row], projection.album[row]))
            .or_insert((row as u32, 0));
        entry.1 += 1;
    }
    albums
}

/// The albums whose track numbers don't add up, keyed by (album artist,
/// album, disc). An album is flagged when a track carries no number at all,
/// or when the numbers stop short of their own highest. Both are the same
/// complaint: the album can't be played in the order it was released in.
///
/// Track numbers rather than ids while grouping, two bytes a row: the ids
/// for the flagged albums come from a second walk, which costs a pass over a
/// column and saves ten bytes a row on a ten-million-row library.
///
/// Rows with no album name are skipped outright. There's no order to be out
/// of: a loose single isn't track 4 of anything, and folding every one of
/// them into the empty symbol built a pseudo-album that was flagged by
/// construction, since a track with no album usually has no number either.
/// That put every album-less track in the library behind the tile's
/// drill-down permanently, which is a complaint nobody can act on.
fn gap_keys(projection: &Projection) -> HashSet<(u32, u32, u16)> {
    let mut discs: HashMap<(u32, u32, u16), Vec<u16>> = HashMap::new();
    for row in 0..projection.len() {
        if projection.is_dead(row as u32) || !has_album(projection, row) {
            continue;
        }
        discs
            .entry((
                projection.album_artist[row],
                projection.album[row],
                projection.disc_no[row],
            ))
            .or_default()
            .push(projection.track_no[row]);
    }
    discs
        .into_iter()
        .filter(|(_, numbers)| {
            let highest = numbers.iter().copied().max().unwrap_or(0);
            numbers.contains(&0)
                || numbers.iter().copied().collect::<HashSet<_>>().len() < highest as usize
        })
        .map(|(key, _)| key)
        .collect()
}

/// Whether an album's cover probe counts as missing. Settling is present:
/// that state exists so a cover mid-download isn't reported as a hole, and a
/// tile that flagged it would flip back on the next refresh for no reason
/// the user did anything about.
fn art_missing(cover: art::Cover) -> bool {
    matches!(cover, art::Cover::None)
}

/// Whether the album this file stands for has no cover, off the cache when
/// the file and its folder still look the way they did when the cache last
/// answered, and off the disk otherwise. Two stats against a tag parse and
/// a full image read.
fn probe_album(cache: &mut HashMap<String, ArtVerdict>, path: &str) -> bool {
    let file = std::path::Path::new(path);
    let identity = (
        art::identity(file),
        file.parent().map(art::identity).unwrap_or((0, 0)),
    );
    if let Some(verdict) = cache.get(path) {
        if (verdict.file, verdict.folder) == identity {
            return verdict.missing;
        }
    }
    let missing = art_missing(art::cover_art_source(file));
    cache.insert(
        path.to_owned(),
        ArtVerdict {
            file: identity.0,
            folder: identity.1,
            missing,
        },
    );
    missing
}

/// The expensive half, off the UI thread, publishing each answer into
/// `out` as it lands rather than returning all four at the end.
///
/// The order is deliberate: the three column-and-memory answers first, the
/// per-file art probe last, so a user watching the window sees three tiles
/// fill in immediately and one count its way through. A stop between stages
/// leaves the rest unpublished, which is what keeps a half-measured page off
/// the tiles.
fn measure(projection: &Projection, db_path: &std::path::Path, cancel: &AtomicBool, out: &Pass) {
    let stopped = || cancel.load(Ordering::Relaxed);
    // Albums first, since two of the four answers are per album: one entry
    // per (folder, album) with a representative row and how many tracks it
    // covers.
    let albums = group_albums(projection);
    let album_count = albums.len() as u64;
    let flagged = gap_keys(projection);
    if stopped() {
        return out.stop();
    }
    let mut gap_tracks = Offenders::default();
    for row in 0..projection.len() {
        if projection.is_dead(row as u32) {
            continue;
        }
        if flagged.contains(&(
            projection.album_artist[row],
            projection.album[row],
            projection.disc_no[row],
        )) {
            gap_tracks.count += 1;
            gap_tracks.ids.push(projection.db_id[row]);
        }
    }
    let gap_albums = flagged.len() as u64;
    out.land(move |data| {
        data.albums = album_count;
        data.gap_albums = gap_albums;
        data.gap_tracks = gap_tracks;
    });
    if stopped() {
        return out.stop();
    }

    let groups = match_duplicates(projection);
    let dup_groups = groups.len() as u64;
    let dup_tracks = groups.iter().map(|g| g.members.len() as u64).sum();
    out.land(move |data| {
        data.dup_groups = dup_groups;
        data.dup_tracks = dup_tracks;
    });
    if stopped() {
        return out.stop();
    }

    // One connection for the two stages that need one. Without it neither
    // can answer, and the tiles stay blank rather than claiming a zero.
    let Ok(conn) = store::open(db_path) else {
        log::warn!("health: could not open the library database to measure formats and art");
        return out.stop();
    };

    // The container split walks every path, which is a full table scan and
    // no business of the UI thread's.
    let Ok(breakdown) = store::extension_breakdown(&conn) else {
        return out.stop();
    };
    let mut unwritable = Offenders {
        count: breakdown
            .iter()
            .filter(|(ext, _)| !store::extension_writable(ext))
            .map(|(_, count)| count)
            .sum(),
        ids: Vec::new(),
    };
    if unwritable.count > 0 {
        let Ok(ids) = store::unwritable_ids(&conn, DRILL_ALL) else {
            return out.stop();
        };
        unwritable.ids = ids;
    }
    let files = breakdown.iter().map(|(_, count)| count).sum();
    out.land(move |data| {
        data.files = files;
        data.unwritable = unwritable;
    });
    if stopped() {
        return out.stop();
    }

    // The art probe reads tags off one file per album, so it's the pass's
    // whole cost on a library that has never had covers fetched, and the one
    // stage with a count worth showing.
    let representatives: Vec<((u32, u32), i64, u64)> = albums
        .iter()
        .map(|(key, (row, tracks))| (*key, projection.db_id[*row as usize], *tracks))
        .collect();
    drop(albums);
    let ids: Vec<i64> = representatives.iter().map(|(_, id, _)| *id).collect();
    let Ok(paths) = store::paths_by_id(&conn, &ids) else {
        return out.stop();
    };
    out.tick(0, representatives.len());
    let mut art = Offenders::default();
    let mut art_albums = 0u64;
    let mut without_art: HashSet<(u32, u32)> = HashSet::new();
    for (probed, (key, id, tracks)) in representatives.iter().enumerate() {
        if stopped() {
            return out.stop();
        }
        out.tick(probed, representatives.len());
        let Some(path) = paths.get(id) else { continue };
        if with_art_cache(|cache| probe_album(cache, path)) {
            art_albums += 1;
            art.count += tracks;
            without_art.insert(*key);
        }
    }
    // Albums that left the library take their verdicts with them, so a
    // library that churns doesn't grow the cache without bound.
    {
        let current: HashSet<&String> = paths.values().collect();
        with_art_cache(|cache| cache.retain(|path, _| current.contains(path)));
    }
    // The ids behind the art tile, from one more walk of the columns the
    // grouping keyed on.
    for row in 0..projection.len() {
        if projection.is_dead(row as u32) {
            continue;
        }
        if without_art.contains(&(projection.folder[row], projection.album[row])) {
            art.ids.push(projection.db_id[row]);
        }
    }
    out.land(move |data| {
        data.art_albums = art_albums;
        data.art_tracks = art;
    });
}

impl Render for HealthWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The page renders under the player's art tint like the workspace that
        // opened it, and claims the widget theme while it holds focus, the
        // stats window's move.
        let player = self.state.player.entity_id();
        palette::note_focus(player, window.is_window_active(), cx);
        panel::window_body(player, || {
            let sections = div()
                .flex()
                .flex_col()
                .gap(SECTION_GAP)
                .child(self.overview_section(cx))
                .child(self.tagging_section(cx))
                .child(self.audio_section(cx))
                .child(self.files_section(cx));
            // The probe sits beside the sections rather than inside their
            // column: an absolutely positioned child is out of the flex flow,
            // so it measures the same box without earning a section gap of
            // its own.
            let page = div()
                .relative()
                .w_full()
                .child(self.width_probe(cx))
                .child(sections);
            div()
                .size_full()
                .flex()
                .flex_row()
                .bg(palette::bg_elevated())
                .text_color(palette::text_bright())
                .text_sm()
                // The backdrop paints first, under the page; without it
                // translucent surfaces would sink into the window's own black
                // instead of the playing track's art.
                .children(self.backdrop.layer(&self.state.now_art, window, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .bg(palette::bg_elevated())
                        .child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .relative()
                                .child(
                                    div()
                                        .id("health-page")
                                        .size_full()
                                        .overflow_y_scroll()
                                        .track_scroll(&self.scroll)
                                        .p(tokens::SPACE_MD)
                                        // Room for the scrollbar's 16px lane,
                                        // so the tiles' controls never end up
                                        // under the thumb.
                                        .pr(tokens::SPACE_MD + px(16.))
                                        .child(page),
                                )
                                // Fades out when idle, same as the panels.
                                .child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .child(Scrollbar::vertical(&self.scroll)),
                                ),
                        ),
                )
                // The start prompt floats over the page on its own occluding
                // layer, last so it paints on top.
                .children(pass_prompt::overlay(self, window, cx))
                .into_any_element()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rox_library::projection::FilterSet;
    use rox_library::rusqlite::Connection;
    use rox_library::{store, TrackRow};

    /// One row with the fields the health scans read; everything else stays
    /// at its neutral default, which is what an untagged file scans as.
    fn track(path: &str, album: &str, disc_no: u16, track_no: u16) -> TrackRow {
        TrackRow {
            path: path.into(),
            sub: 0,
            cue: None,
            title: "Song".into(),
            artist: "Artist".into(),
            album_artist: "Artist".into(),
            album: album.into(),
            title_sort: String::new(),
            artist_sort: String::new(),
            album_artist_sort: String::new(),
            album_sort: String::new(),
            genre: String::new(),
            year: 0,
            disc_no,
            track_no,
            duration_ms: 200_000,
            codec: "mp3".into(),
            bitrate_kbps: 320,
            sample_rate_hz: 44100,
            bit_depth: 0,
            rating: 0,
            replay_gain: Default::default(),
            bpm: None,
            size: 0,
            mtime: 0,
        }
    }

    /// A projection over an in-memory database seeded with the rows, the same
    /// path the app builds its read model over.
    fn projection(rows: &[TrackRow]) -> Projection {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, rows).unwrap();
        Projection::load_serial(&conn, false).unwrap()
    }

    /// Three albums: one numbered end to end, one missing its second track,
    /// one whose files carry no numbers at all. Exactly the last two are the
    /// ones a user can't play in release order, so exactly those flag.
    #[test]
    fn album_gaps_flag_holes_and_unnumbered_tracks() {
        let p = projection(&[
            track("/m/whole/1.mp3", "Whole", 1, 1),
            track("/m/whole/2.mp3", "Whole", 1, 2),
            track("/m/whole/3.mp3", "Whole", 1, 3),
            track("/m/holed/1.mp3", "Holed", 1, 1),
            track("/m/holed/3.mp3", "Holed", 1, 3),
            track("/m/bare/a.mp3", "Bare", 1, 0),
            track("/m/bare/b.mp3", "Bare", 1, 0),
        ]);
        let flagged = gap_keys(&p);
        let named: HashSet<&str> = flagged
            .iter()
            .map(|(_, album, _)| p.albums.strings[*album as usize].as_str())
            .collect();
        assert_eq!(named, HashSet::from(["Holed", "Bare"]));

        // A disc is its own run of numbers: disc two starting at one again is
        // a complete set, not a hole under disc one's highest.
        let two_discs = projection(&[
            track("/m/set/1-1.mp3", "Set", 1, 1),
            track("/m/set/1-2.mp3", "Set", 1, 2),
            track("/m/set/2-1.mp3", "Set", 2, 1),
        ]);
        assert!(gap_keys(&two_discs).is_empty());
    }

    /// A folder of loose singles is not an album. They all share the empty
    /// album symbol, so keying on it made them one pseudo-album carrying no
    /// track numbers: flagged every time, with every one of its tracks
    /// pinned as an offender, and judged for art by whichever file the
    /// grouping happened to seat first.
    #[test]
    fn album_less_tracks_are_not_one_pseudo_album() {
        let p = projection(&[
            track("/m/whole/1.mp3", "Whole", 1, 1),
            track("/m/whole/2.mp3", "Whole", 1, 2),
            track("/m/singles/a.mp3", "", 0, 0),
            track("/m/singles/b.mp3", "", 0, 0),
            track("/m/singles/c.mp3", "", 0, 0),
        ]);
        assert!(
            gap_keys(&p).is_empty(),
            "the numbered album is whole and the loose files have no order to be out of"
        );

        let albums = group_albums(&p);
        assert_eq!(albums.len(), 1, "only the named album is an album");
        assert_eq!(
            albums.values().next().unwrap().1,
            2,
            "and it covers its own two tracks, not the singles beside it"
        );
    }

    /// Settling is present and None is missing, the whole point of the
    /// distinction: a cover mid-download must not be reported as a hole.
    #[test]
    fn settling_art_is_not_missing_art() {
        assert!(art_missing(art::Cover::None));
        assert!(!art_missing(art::Cover::Settling));
        assert!(!art_missing(art::Cover::Found {
            bytes: vec![0u8; 4],
            mime: "image/png".into(),
            source: art::ArtSource::Embedded,
        }));
    }

    /// A verdict is trusted while the file and its folder still stat the way
    /// they did when it was reached, and thrown out the moment either moves:
    /// a cached answer that contradicts the disk comes back unchanged on a
    /// matching identity, and is corrected on a stale one.
    #[test]
    fn the_art_probe_trusts_a_verdict_until_the_disk_moves() {
        let dir = std::env::temp_dir().join(format!(
            "rox-health-art-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("01.mp3");
        std::fs::write(&file, b"not a real track").unwrap();
        let path = file.to_str().unwrap().to_owned();
        let identity = (art::identity(&file), art::identity(&dir));

        // A bare folder has no art, and the probe says so and remembers.
        let mut cache = HashMap::new();
        assert!(probe_album(&mut cache, &path));
        let remembered = cache[&path];
        assert_eq!((remembered.file, remembered.folder), identity);
        assert!(remembered.missing);

        // The same identity with the opposite verdict: the cache wins, so the
        // disk wasn't read.
        cache.insert(
            path.clone(),
            ArtVerdict {
                file: identity.0,
                folder: identity.1,
                missing: false,
            },
        );
        assert!(!probe_album(&mut cache, &path));

        // A folder that has moved on is reprobed, and the cache corrected.
        cache.insert(
            path.clone(),
            ArtVerdict {
                file: identity.0,
                folder: (identity.1 .0 + 1, identity.1 .1),
                missing: false,
            },
        );
        assert!(probe_album(&mut cache, &path));
        assert!(cache[&path].missing);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The albums the art probe asks about: one entry per folder-and-album
    /// with the tracks it covers, so the tile can say what a missing cover
    /// costs as well as how many albums have none.
    #[test]
    fn albums_group_by_folder_and_name() {
        let p = projection(&[
            track("/m/a/1.mp3", "One", 1, 1),
            track("/m/a/2.mp3", "One", 1, 2),
            track("/m/b/1.mp3", "Two", 1, 1),
        ]);
        let albums = group_albums(&p);
        assert_eq!(albums.len(), 2);
        let mut covered: Vec<u64> = albums.values().map(|(_, tracks)| *tracks).collect();
        covered.sort();
        assert_eq!(covered, [1, 2]);
    }

    /// The door: the offenders a cheap scan collects, pinned through
    /// [`FilterSet::with_ids`], narrow the library to exactly those rows and
    /// nothing else.
    #[test]
    fn the_offender_ids_narrow_to_exactly_their_rows() {
        let mut tagged = track("/m/a/1.mp3", "One", 1, 1);
        tagged.genre = "Shoegaze".into();
        let p = projection(&[
            tagged,
            track("/m/a/2.mp3", "One", 1, 2),
            track("/m/a/3.mp3", "One", 1, 3),
        ]);
        let data = health::completeness(&p, DRILL_ALL);
        assert_eq!(data.tracks, 3);
        let genre = data.missing(Check::Genre);
        assert_eq!(genre.count, 2, "two of the three carry no genre");

        let mask = p
            .filter_mask(&FilterSet::with_ids(genre.ids.clone()))
            .expect("an id pin is a filter");
        let matched: Vec<i64> = (0..p.len())
            .filter(|row| mask[*row])
            .map(|row| p.db_id[row])
            .collect();
        let mut wanted = genre.ids.clone();
        wanted.sort();
        let mut matched = matched;
        matched.sort();
        assert_eq!(matched, wanted);
    }

    /// The staged loading state: a tile whose stage hasn't run says so, the
    /// one running says what it's doing, and the ones behind it hold their
    /// numbers. The whole point is that a tile never shows a zero from a
    /// stage that never ran.
    #[test]
    fn each_tile_reads_its_own_stage_rather_than_the_passs() {
        let fresh = PassState::default();
        assert!(matches!(fresh.cell(Stage::Gaps), Cell::Running { .. }));
        assert!(matches!(fresh.cell(Stage::Art), Cell::Waiting));
        assert!(!fresh.finished());

        // Three stages in: the first two hold numbers, formats is counting,
        // art still hasn't started.
        let midway = PassState {
            landed: 2,
            done: 412,
            total: 1870,
            stopped: false,
        };
        assert!(matches!(midway.cell(Stage::Gaps), Cell::Landed));
        assert!(matches!(midway.cell(Stage::Duplicates), Cell::Landed));
        assert!(matches!(
            midway.cell(Stage::Formats),
            Cell::Running {
                done: 412,
                total: 1870
            }
        ));
        assert!(matches!(midway.cell(Stage::Art), Cell::Waiting));

        // A pass that gave up leaves the stage it was on waiting, not
        // running and not landed, and stops the window's timer.
        let gave_up = PassState {
            landed: 2,
            stopped: true,
            ..Default::default()
        };
        assert!(matches!(gave_up.cell(Stage::Duplicates), Cell::Landed));
        assert!(matches!(gave_up.cell(Stage::Formats), Cell::Waiting));
        assert!(gave_up.finished());

        let done = PassState {
            landed: STAGES.len(),
            ..Default::default()
        };
        assert!(matches!(done.cell(Stage::Art), Cell::Landed));
        assert!(done.finished());
    }

    /// Lanes come off the page width rather than a constant, and the page
    /// only ever draws one, two or four of them. The breakpoints sit exactly
    /// where another tile at its minimum, plus the gap in front of it, stops
    /// fitting; the off-by-one at that edge is the only thing here worth a
    /// test, and the thing this guards against now is a three that slips
    /// back in. Whether four tiles look right at 1100px is Andrew's eyes.
    #[test]
    fn lanes_are_one_two_or_four() {
        let gap = f32::from(tokens::SPACE_SM);
        let exact = |n: usize| MIN_TILE_W * n as f32 + gap * (n - 1) as f32;

        assert_eq!(columns_for(exact(1)), 1);
        assert_eq!(columns_for(exact(2)), 2, "two tiles exactly fit two lanes");
        assert_eq!(
            columns_for(exact(2) - 1.),
            1,
            "a pixel short of two lanes drops to one"
        );
        assert_eq!(
            columns_for(exact(4)),
            MAX_TILE_COLS,
            "four tiles exactly fit four lanes"
        );
        assert_eq!(
            columns_for(exact(4) - 1.),
            2,
            "a pixel short of four lanes steps back to two, never three"
        );

        // The whole band where a third tile fits but a fourth doesn't stays
        // at two, which is the point of the rule.
        assert_eq!(columns_for(exact(3)), 2);
        assert_eq!(columns_for(exact(3) + 40.), 2);
        for w in (0..2000).step_by(7) {
            let lanes = columns_for(w as f32);
            assert!(matches!(lanes, 1 | 2 | 4), "{w}px asked for {lanes} lanes");
        }

        // A page too narrow for one tile still gets one: a squeezed tile
        // beats an empty section. A page wider than the cap keeps four.
        assert_eq!(columns_for(0.), 1);
        assert_eq!(columns_for(-50.), 1);
        assert_eq!(columns_for(f32::NAN), 1);
        assert_eq!(columns_for(10_000.), MAX_TILE_COLS);
    }

    /// The count column is sized off the widest thing a row can say, so the
    /// meters in front of it all end on the same x. Character counting is
    /// the estimate it rests on: a bigger library never gets a narrower
    /// column, a CJK glyph claims the width of two Latin ones, and the
    /// column never drops under its floor whatever the locale says.
    #[test]
    fn the_count_column_holds_the_widest_count() {
        assert_eq!(text_units("29,629 missing"), 14.);
        assert_eq!(text_units("不足なし"), 8., "square glyphs count double");

        assert!(count_column_w(0) >= CHECK_COUNT_MIN_W);
        assert!(
            count_column_w(10_000_000) >= count_column_w(9),
            "a longer count never gets a shorter column"
        );
        assert!(
            count_column_w(10_000_000) > CHECK_COUNT_MIN_W,
            "an eight-character count plus a word outgrows the floor"
        );
    }

    /// Sort-name coverage is over distinct values, not rows: one artist with
    /// a sort name and one without is half, however many tracks each has.
    /// The empty name stays out of both halves.
    #[test]
    fn sort_coverage_counts_values_rather_than_rows() {
        let mut sorted = track("/m/a/1.mp3", "One", 1, 1);
        sorted.artist = "崎山蒼志".into();
        sorted.artist_sort = "Sakiyama Soushi".into();
        let mut bare = track("/m/b/1.mp3", "Two", 1, 1);
        bare.artist = "Slowdive".into();
        let mut bare_again = track("/m/b/2.mp3", "Two", 1, 2);
        bare_again.artist = "Slowdive".into();
        let p = projection(&[sorted, bare, bare_again]);
        let data = scan_projection(&p);
        assert_eq!(data.sort.artists, (1, 2));
        assert_eq!(
            data.sort_offenders.count, 2,
            "the door counts rows, since that's what the library view shows"
        );
    }
}
