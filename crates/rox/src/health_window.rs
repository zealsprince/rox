//! The library health window: how well the library is tagged and what to
//! fix next. Every number is a door into its offending tracks, and fixable
//! ones offer the fix rox already has.
//!
//! The headline is the plain share of live tracks carrying all five core
//! tags, from [`rox_library::health`] so the transport widget can't disagree.
//!
//! Two cost classes, per ADR 11's read cadence: SQL aggregates and projection
//! walks refresh on open and on catalog changes, never per frame; the
//! disk-bound checks run as one background pass whose stages publish as they
//! land, so three tiles fill at once while the art probe counts through.
//!
//! Nothing here writes a file. Fixes are existing confirmed steps (ADR 14),
//! and drill-downs open their own window rather than touching the app-wide
//! query.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{
    AnyElement, App, Bounds, Context, Div, FocusHandle, FontWeight, Global, ScrollHandle,
    SharedString, Stateful, Subscription, Task, Window, WindowHandle, div, prelude::*, px,
    relative, size,
};
use gpui_component::Root;
use gpui_component::scroll::Scrollbar;

use rox_core::settings::Settings;
use rox_design::assets::icons;
use rox_design::{palette, tokens};
use rox_library::duplicates::match_duplicates;
use rox_library::health::{self, Check};
use rox_library::projection::Projection;
use rox_library::{art, store};
use rox_panel_api::charts;
use rox_panel_api::panel::{self, AppState};
use rox_panel_kit::ui::{self as settings_ui, SECTION_GAP, section};
use rox_services::backdrop::WindowBackdrop;
use rox_services::catalog::LibraryEvent;

use crate::pass_prompt;
use crate::quick_play;

/// No cap on a drill-down's ids: the filter pin is a set, so size costs a
/// hash lookup a row. `i64::MAX` because one call spends it as a SQL `LIMIT`.
const DRILL_ALL: usize = i64::MAX as usize;

/// A catalog change fires at both ends of the reload, and a scan per batch;
/// long enough to swallow the pair, short enough that nothing looks stuck.
const SCAN_DEBOUNCE: Duration = Duration::from_millis(200);

/// Fits the longest caption ("N tagged, N measured, N missing") on one line.
const MIN_TILE_W: f32 = 260.;

/// About sixty characters, the measure prose reads at on a full-width tile.
const DESC_MAX_W: f32 = 420.;

const TILE_ICON: f32 = 14.;

/// Past four the descriptions become two-word columns.
const MAX_TILE_COLS: usize = 4;

/// The default window's two lanes; only wrong for the first frame.
const ASSUMED_CONTENT_W: f32 = 640.;

const RING_SIZE: f32 = 108.;
const RING_THICKNESS: f32 = 13.;

/// Fixed so the five coverage bars start at the same x.
const CHECK_LABEL_WIDTH: f32 = 64.;

/// Fixed-width count column so every bar ends on the same x. Sized off the
/// widest string, rounding up: spare pixels cost nothing, short ones misalign.
const CHECK_COUNT_MIN_W: f32 = 64.;
const CHECK_COUNT_CHAR_W: f32 = 6.5;

struct OpenHealth(WindowHandle<Root>);

impl Global for OpenHealth {}

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

/// The ids are the door into a power search over exactly those tracks.
#[derive(Clone, Default)]
struct Offenders {
    count: u64,
    ids: Vec<i64>,
}

/// (with, total) per table. Sort names ride values, so artist and album
/// shares are over distinct names; titles aren't interned, so over rows.
#[derive(Clone, Copy, Default)]
struct SortCoverage {
    artists: (u64, u64),
    album_artists: (u64, u64),
    albums: (u64, u64),
    titles: (u64, u64),
}

/// 100% for an empty table: nothing is missing.
fn share(with: u64, total: u64) -> f64 {
    if total == 0 {
        return 100.;
    }
    (with as f64 / total as f64 * 100.).round()
}

#[derive(Default)]
struct HealthData {
    /// From the library crate, so the ring, tiles, and transport widget agree.
    complete: health::Completeness,
    /// Deliberately not a core tag: a taste judgement isn't missing metadata.
    rating: Offenders,
    sort: SortCoverage,
    sort_offenders: Offenders,
    gain: store::GainCoverage,
    bpm: store::BpmCoverage,
    acoustic: rox_library::embeddings::Coverage,
}

/// Tiles read their stage, not these zeros, to know whether they have an answer.
#[derive(Clone, Default)]
struct PassData {
    art_albums: u64,
    albums: u64,
    art_tracks: Offenders,
    dup_groups: u64,
    dup_tracks: u64,
    gap_albums: u64,
    gap_tracks: Offenders,
    unwritable: Offenders,
    files: u64,
}

/// Cached per album: the probe is the pass's entire cost and reruns on every
/// library event. Valid while the file and folder identities hold, since an
/// embedded cover changes the file and a dropped cover.jpg the folder.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ArtVerdict {
    file: (i64, i64),
    folder: (i64, i64),
    missing: bool,
}

/// Process-wide, so it survives a cancel and the window closing.
static ART_CACHE: Mutex<Option<HashMap<String, ArtVerdict>>> = Mutex::new(None);

/// Locked per call, so a stopped pass never holds the map against its successor.
fn with_art_cache<R>(f: impl FnOnce(&mut HashMap<String, ArtVerdict>) -> R) -> R {
    let mut guard = ART_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    f(guard.get_or_insert_with(HashMap::new))
}

const PASS_TICK: Duration = Duration::from_millis(250);

/// In run order. Art is last: it's the only file-touching stage, and the
/// others land in a second or two instead of waiting behind it.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Gaps,
    Duplicates,
    Formats,
    Art,
}

const STAGES: [Stage; 4] = [Stage::Gaps, Stage::Duplicates, Stage::Formats, Stage::Art];

impl Stage {
    fn position(self) -> usize {
        STAGES.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// Only the art probe has a count; the rest are single column walks.
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

/// A sample of a running pass, taken together so a repaint pairs markers and
/// numbers consistently. Order matters: the marker is read before the
/// revision, and [`Pass::land`] publishes in the opposite order, so a landed
/// marker never pairs with stale data.
#[derive(Clone, Copy, Default)]
struct PassState {
    landed: usize,
    done: u64,
    total: u64,
    stopped: bool,
}

impl PassState {
    fn finished(&self) -> bool {
        self.stopped || self.landed >= STAGES.len()
    }

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

enum Cell {
    Waiting,
    /// `total` is zero for a stage with nothing to count.
    Running {
        done: u64,
        total: u64,
    },
    Landed,
}

/// Sampled on a timer rather than pushed through the entity, which would
/// need a handle across four stages and a cancel.
#[derive(Default)]
struct Pass {
    /// Also the index of the running stage.
    landed: AtomicUsize,
    done: AtomicUsize,
    total: AtomicUsize,
    /// So the window copies `data` once per landing, not per tick.
    revision: AtomicU64,
    /// Stops the timer and leaves unrun stages waiting rather than zero.
    stopped: AtomicBool,
    data: Mutex<PassData>,
}

impl Pass {
    /// The revision moves before the marker: the half of the ordering [`PassState`] needs.
    fn land(&self, fill: impl FnOnce(&mut PassData)) {
        fill(&mut self.data.lock().unwrap());
        self.done.store(0, Ordering::Relaxed);
        self.total.store(0, Ordering::Relaxed);
        self.revision.fetch_add(1, Ordering::Relaxed);
        self.landed.fetch_add(1, Ordering::Relaxed);
    }

    fn tick(&self, done: usize, total: usize) {
        self.done.store(done, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }

    /// Later tiles stay blank rather than claiming a zero never measured.
    fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
    }

    fn revision(&self) -> u64 {
        self.revision.load(Ordering::Relaxed)
    }

    fn snapshot(&self) -> PassData {
        self.data.lock().unwrap().clone()
    }

    /// Marker first.
    fn state(&self) -> PassState {
        PassState {
            landed: self.landed.load(Ordering::Relaxed),
            stopped: self.stopped.load(Ordering::Relaxed),
            done: self.done.load(Ordering::Relaxed) as u64,
            total: self.total.load(Ordering::Relaxed) as u64,
        }
    }
}

/// Taken on the UI thread so the walk never touches the entity.
struct Inputs {
    gain: store::GainCoverage,
    bpm: store::BpmCoverage,
    acoustic: rox_library::embeddings::Coverage,
    projection: Option<Arc<Projection>>,
}

struct HealthWindow {
    state: AppState,
    data: HealthData,
    pass: PassData,
    /// Sampled with `pass`, never read live.
    cells: PassState,
    cancel: Arc<AtomicBool>,
    /// Drops a result already on its way back when the cancel went up.
    generation: u64,
    /// Held, so a burst of events replaces it and closing the window drops it.
    scan: Option<Task<()>>,
    scan_generation: u64,
    prompt: Option<pass_prompt::Prompt>,
    /// Measured by a paint-time probe, the only way an element learns its size.
    /// One number for the whole page, so every section's columns line up.
    content_width: f32,
    value_edit: panel::ValueEdit,
    dialog_focus: FocusHandle,
    scroll: ScrollHandle,
    backdrop: WindowBackdrop,
    _library_changed: Subscription,
    _backdrop_changed: Subscription,
}

/// The pass holds no handle back here, so without this it would keep probing
/// for a closed window. The walk is a `Task` and drops with the window.
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
        // The OS close button never runs remove_window, so the size persists here.
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

    /// Measure the cheap half off the UI thread, then start the pass. The walks
    /// are tens of milliseconds on a large library, too much for the UI thread
    /// at twice per edit. Old numbers stay up until new ones land, and a stale
    /// result drops by generation.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.scan_generation += 1;
        let generation = self.scan_generation;
        // The first walk has nothing to flicker, so it skips the debounce.
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

    fn start_pass(&mut self, projection: Option<Arc<Projection>>, cx: &mut Context<Self>) {
        // A fresh flag, so the next cancel can't reach back and stop this pass.
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
        // The generation guard is the loop's only stop: only another start_pass
        // cancels, and it bumps the generation first.
        cx.spawn(async move |this, cx| {
            let mut copied = 0;
            loop {
                cx.background_executor().timer(PASS_TICK).await;
                let running = this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return false;
                    }
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

    fn columns(&self) -> usize {
        columns_for(self.content_width)
    }

    /// Reports the page's width from the paint, so the next frame lays out with
    /// it. Wakes only when the lane count changes, or it would repaint forever.
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

    /// In a window of its own, not the app-wide query, so a look doesn't cost the
    /// user their library view. The window is a singleton.
    fn show(&mut self, door: &Door, ids: &[i64], caption: SharedString, cx: &mut Context<Self>) {
        match door {
            Door::Ids => {
                let seed = quick_play::Seed {
                    ids: ids.to_vec(),
                    label: caption,
                };
                crate::search_window::open_seeded(self.state.clone(), seed, cx);
            }
            // A query term stays true as the library changes; an id pin is a snapshot.
            Door::Field(field) => {
                crate::search_window::open_with_query(self.state.clone(), &format!("-{field}"), cx)
            }
        }
    }

    fn start_pass_prompt(&mut self, pass: pass_prompt::Pass, cx: &mut Context<Self>) {
        let library = self.state.library.clone();
        pass_prompt::raise(self, pass, library, cx);
    }

    /// The ring is complete against incomplete only: per-check slices would
    /// double-count tracks missing two tags. The rows carry the breakdown.
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
                    // Over the hole in a div: canvas text would need the text system wired through.
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

    /// The count width is passed in so all five rows' meters end on the same x.
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
        // The tile counts what Analyze Missing would run. The refused pile goes in
        // the caption, or a 0 would hide thousands of untimed tracks.
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

    /// Two doors, the drill-down and the tagger, so it can't use
    /// [`Self::missing_tile`]'s one.
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

    /// Counted by extension via [`store::WRITABLE_EXTENSIONS`], while the
    /// writer decides per file: a fragmented MP4 with absolute offsets is
    /// refused even as `.m4a`, and catching those would mean opening every
    /// file. So this reads as "formats rox can write", not "files it will retag".
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

    fn sort_tile(&self, cx: &mut Context<Self>) -> AnyElement {
        let sort = self.data.sort;
        // Fill first: it's what the tile is for.
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

    /// The title becomes the caption over the window, since an unseen filter
    /// just looks like missing rows.
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

    /// An unrun stage shows a dash: a zero would be good news it hasn't earned.
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

/// An id pin, or an absence term the user can read and edit in the search box.
#[derive(Clone)]
enum Door {
    Ids,
    Field(&'static str),
}

fn count_value(count: u64) -> SharedString {
    SharedString::from(rox_i18n::format::format_int(count as i64))
}

fn int(count: u64) -> String {
    rox_i18n::format::format_int(count as i64)
}

fn pct(share_of: (u64, u64)) -> String {
    rox_i18n::format::format_percent(share(share_of.0, share_of.1))
}

fn tracks_worded(count: u64) -> String {
    rox_i18n::t!("status-count-tracks", count = count).to_string()
}

fn groups_worded(count: u64) -> String {
    rox_i18n::t!("health-count-groups", count = count).to_string()
}

/// One message so a translator can reorder the halves.
fn seed_caption(source: SharedString, count: u64) -> SharedString {
    rox_i18n::t!(
        "search-seed-caption",
        source = source.to_string(),
        count = tracks_worded(count),
    )
}

fn albums_worded(count: u64) -> String {
    rox_i18n::t!("status-count-albums", count = count).to_string()
}

/// The description says what the number counts: "82" over "Album Art" is
/// a guess until it does. Content and action are split by `justify_between`
/// so buttons line up along a stretched lane's bottom edge.
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

/// One, two, or four lanes, never three: three leaves four-tile sections with
/// a lone ragged tile. Pure for testing the edge off-by-one. NaN and widths
/// too narrow for one tile get one.
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

/// Estimated from character counts: the real advance is only known in paint,
/// and a few spare pixels cost nothing.
fn count_column_w(total: u64) -> f32 {
    let widest = text_units(&rox_i18n::t!("health-complete")).max(text_units(&rox_i18n::t!(
        "health-overview-missing",
        missing = int(total)
    )));
    (widest * CHECK_COUNT_CHAR_W).ceil().max(CHECK_COUNT_MIN_W)
}

/// CJK glyphs count as two, the only class wide enough to matter here.
fn text_units(text: &str) -> f32 {
    text.chars()
        .map(|c| if c >= '\u{2e80}' { 2. } else { 1. })
        .sum()
}

/// Every tile is `flex_1` with no fillers, so short lanes stretch rather than
/// look half empty. No `align_items`: taffy's default stretch gives every
/// tile in a lane the tallest one's height.
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

/// The rating column and sort tables; the five core tags are
/// [`health::completeness`]'s own walk. Tombstoned rows are skipped.
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

/// The empty value counts in neither half: a nameless artist never has a sort name.
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

fn check_label(check: Check) -> SharedString {
    match check {
        Check::Title => rox_i18n::t!("health-tile-title"),
        Check::Artist => rox_i18n::t!("health-tile-artist"),
        Check::Album => rox_i18n::t!("health-tile-album"),
        Check::Genre => rox_i18n::t!("health-tile-genre"),
        Check::Year => rox_i18n::t!("health-tile-year"),
    }
}

fn check_key(check: Check) -> &'static str {
    match check {
        Check::Title => "title",
        Check::Artist => "artist",
        Check::Album => "album",
        Check::Genre => "genre",
        Check::Year => "year",
    }
}

/// Genre and year have query terms (`-genre`); the rest pin ids.
fn check_door(check: Check) -> Door {
    match check {
        Check::Genre => Door::Field("genre"),
        Check::Year => Door::Field("year"),
        _ => Door::Ids,
    }
}

/// Every album-less row shares the empty symbol, so keying on the album
/// column folds them into one bucket unless it checks this.
fn has_album(projection: &Projection, row: usize) -> bool {
    !projection.albums.strings[projection.album[row] as usize].is_empty()
}

/// One entry per (folder, album) with a representative row and its track
/// count: art sits beside the files. Loose singles stay out, or one file's
/// verdict would stand for the whole folder, and a per-track probe is the
/// cost this pass avoids.
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

/// (album artist, album, disc) where a track has no number or the numbers
/// stop short of their highest. Loose singles are skipped: they'd form a
/// pseudo-album flagged by construction.
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

/// Settling is present: a cover mid-download isn't a hole.
fn art_missing(cover: art::Cover) -> bool {
    matches!(cover, art::Cover::None)
}

/// Off the cache while the identities hold: two stats against a tag parse and image read.
fn probe_album(cache: &mut HashMap<String, ArtVerdict>, path: &str) -> bool {
    let file = std::path::Path::new(path);
    let identity = (
        art::identity(file),
        file.parent().map(art::identity).unwrap_or((0, 0)),
    );
    if let Some(verdict) = cache.get(path)
        && (verdict.file, verdict.folder) == identity
    {
        return verdict.missing;
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

/// Publishes each answer as it lands, art last. A stop between stages leaves
/// the rest unpublished rather than half-measured.
fn measure(projection: &Projection, db_path: &std::path::Path, cancel: &AtomicBool, out: &Pass) {
    let stopped = || cancel.load(Ordering::Relaxed);
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

    // No connection: the tiles stay blank rather than claim a zero.
    let Ok(conn) = store::open(db_path) else {
        log::warn!("health: could not open the library database to measure formats and art");
        return out.stop();
    };

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
    // Prune departed albums so the cache doesn't grow without bound.
    {
        let current: HashSet<&String> = paths.values().collect();
        with_art_cache(|cache| cache.retain(|path, _| current.contains(path)));
    }
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
            // Absolute, so the probe measures the box without earning a section gap.
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
                                        .pr(tokens::SPACE_MD + px(16.))
                                        .child(page),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .child(Scrollbar::vertical(&self.scroll)),
                                ),
                        ),
                )
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
    use rox_library::{TrackRow, store};

    fn track(path: &str, album: &str, disc_no: u16, track_no: u16) -> TrackRow {
        TrackRow {
            remote_url: String::new(),
            remote_live: false,
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

    fn projection(rows: &[TrackRow]) -> Projection {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, rows).unwrap();
        Projection::load_serial(&conn, false).unwrap()
    }

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

        let two_discs = projection(&[
            track("/m/set/1-1.mp3", "Set", 1, 1),
            track("/m/set/1-2.mp3", "Set", 1, 2),
            track("/m/set/2-1.mp3", "Set", 2, 1),
        ]);
        assert!(gap_keys(&two_discs).is_empty());
    }

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

        let mut cache = HashMap::new();
        assert!(probe_album(&mut cache, &path));
        let remembered = cache[&path];
        assert_eq!((remembered.file, remembered.folder), identity);
        assert!(remembered.missing);

        cache.insert(
            path.clone(),
            ArtVerdict {
                file: identity.0,
                folder: identity.1,
                missing: false,
            },
        );
        assert!(!probe_album(&mut cache, &path));

        cache.insert(
            path.clone(),
            ArtVerdict {
                file: identity.0,
                folder: (identity.1.0 + 1, identity.1.1),
                missing: false,
            },
        );
        assert!(probe_album(&mut cache, &path));
        assert!(cache[&path].missing);

        std::fs::remove_dir_all(&dir).unwrap();
    }

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

    #[test]
    fn each_tile_reads_its_own_stage_rather_than_the_passs() {
        let fresh = PassState::default();
        assert!(matches!(fresh.cell(Stage::Gaps), Cell::Running { .. }));
        assert!(matches!(fresh.cell(Stage::Art), Cell::Waiting));
        assert!(!fresh.finished());

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

        assert_eq!(columns_for(exact(3)), 2);
        assert_eq!(columns_for(exact(3) + 40.), 2);
        for w in (0..2000).step_by(7) {
            let lanes = columns_for(w as f32);
            assert!(matches!(lanes, 1 | 2 | 4), "{w}px asked for {lanes} lanes");
        }

        assert_eq!(columns_for(0.), 1);
        assert_eq!(columns_for(-50.), 1);
        assert_eq!(columns_for(f32::NAN), 1);
        assert_eq!(columns_for(10_000.), MAX_TILE_COLS);
    }

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
