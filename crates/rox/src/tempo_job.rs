//! The tempo measurement pass: estimate a BPM for every track without one.
//! The estimator lives in [`rox_acoustic::tempo`], the work list and write in
//! [`rox_library::store`]; this module is the app-global bookkeeping around
//! a running pass, the same shape as [`crate::replaygain_job`].
//!
//! Tempos only go to the database, never the files: that would rewrite audio
//! files to record an estimate, and a cue subsong has no file of its own.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{App, Entity, Global};

use rox_library::store;

use rox_core::settings::Settings;
use rox_services::catalog::{Library, LibraryJob};

const PACE_FLOOR: usize = 16;

/// Batched so the write lock isn't taken per track; small enough that a
/// stopped pass keeps nearly everything it measured.
const BATCH: usize = 32;

/// Zero total means the work list is still being built.
#[derive(Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    /// Counts undecodable files and uncallable beats; the log tells them apart.
    failed: AtomicUsize,
    /// Whichever worker wrote last: a sample, not a queue position.
    current: Mutex<String>,
    cancel: AtomicBool,
    /// Started once the work list is built, so the query doesn't bill the first
    /// track.
    pace: rox_core::pace::Pace,
}

impl Progress {
    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    pub fn failed(&self) -> usize {
        self.failed.load(Ordering::Relaxed)
    }

    pub fn current(&self) -> String {
        self.current.lock().unwrap().clone()
    }

    pub fn stopping(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// None until enough have finished for the average to mean anything.
    pub fn secs_per_track(&self) -> Option<f64> {
        self.pace.secs_per_track(self.done())
    }

    pub fn eta_secs(&self) -> Option<f64> {
        self.pace.eta_secs(self.done(), self.total())
    }

    fn keep_going(&self) -> bool {
        !self.cancel.load(Ordering::Relaxed)
    }
}

/// App-global so the pass outlives the window that started it.
#[derive(Default)]
struct Running(Option<Arc<Progress>>);

impl Global for Running {}

pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel.store(true, Ordering::Relaxed);
    }
}

/// A no-op while a pass runs or the feature is off.
///
/// `retry_refused` runs only the tracks an earlier pass refused, for an
/// improved estimator; off, it runs the tracks nothing has heard. The two
/// lists never overlap.
///
/// Safe to call inside the library's own update: nothing reads the entity
/// until the spawned task, and reading a leased entity panics.
pub fn start(library: Entity<Library>, retry_refused: bool, cx: &mut App) {
    let settings = Settings::load();
    if progress(cx).is_some() || !settings.tempo_analysis {
        return;
    }
    // Read once: a pass keeps the pool it started with.
    let workers = settings.tempo_workers.max(1);
    let progress = Arc::new(Progress::default());
    cx.set_global(Running(Some(progress.clone())));
    // Nothing observes an app-global pass, so tick the menubar and tasks window.
    crate::tasks_window::repaint_while_running(cx);
    // Quit raises the stop flag, so workers stop on a batch boundary rather
    // than mid-write.
    cx.on_app_quit({
        let progress = progress.clone();
        move |_| {
            progress.cancel.store(true, Ordering::Relaxed);
            async {}
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        // Read here rather than up top to keep a caller inside its update safe.
        let Ok(db_path) = cx.update(|cx| library.read(cx).db_path()) else {
            return;
        };
        let written = cx
            .background_executor()
            .spawn({
                let progress = progress.clone();
                async move { run(&db_path, workers, retry_refused, &progress) }
            })
            .await;
        cx.update(|cx| {
            cx.set_global(Running(None));
            // Remembered in worker-seconds to price the next run. Only off a decent
            // stretch: a few tracks measure startup, not the rate.
            if progress.done() >= PACE_FLOOR
                && let Some(per) = progress.secs_per_track()
            {
                let pace = (per * workers as f64) as f32;
                Settings::update(move |s| s.session.tempo_pace = pace);
            }
            match written {
                Ok(0) => {}
                Ok(written) => {
                    log::info!("tempo: {written} tracks measured");
                    // The tempos went onto rows the projection holds a packed copy of, so
                    // reload it or the BPM column keeps its blanks.
                    library.update(cx, |library, cx| library.reload_projection(cx));
                }
                Err(e) => {
                    log::error!("tempo: {e}");
                }
            }
        })
        .ok();
    })
    .detach();
}

/// Start a pass on each watch settle while the auto switch is on. The switch
/// is read here, not in [`start`], so the button works with it off. Only the
/// delta comes in this way; the initial backlog goes through the prompt.
pub fn follow(library: &Entity<Library>, cx: &mut App) {
    App::subscribe(cx, library, |library, event, cx| {
        if matches!(event, LibraryJob::WatchSettled) && Settings::load().tempo_auto {
            start(library, false, cx);
        }
    })
    .detach();
}

/// Time a few tracks to price a pass before it runs. Returns worker-seconds
/// per track, the unit [`rox_core::pace::estimate`] divides. Writes nothing.
pub fn measure_pace(db_path: &Path, retry_refused: bool) -> Result<f32, String> {
    let conn = store::open(db_path).map_err(|e| e.to_string())?;
    let work = work_list(&conn, retry_refused)?;
    let picked = rox_core::pace::sample_indices(work.len(), rox_core::pace::PROBE_TRACKS);
    if picked.is_empty() {
        return Err("there's nothing left to measure".into());
    }

    let started = Instant::now();
    let mut timed = 0usize;
    for index in picked {
        let track = &work[index];
        // An uncallable track still cost its decode, which is what's timed.
        let _ = rox_acoustic::tempo::estimate(Path::new(&track.path), track.duration_ms);
        timed += 1;
    }
    Ok((started.elapsed().as_secs_f64() / timed as f64) as f32)
}

/// The retry pile is the difference of two store lists:
/// `bpm_missing(conn, true)` lifts the refusal filter rather than inverting
/// it.
fn work_list(
    conn: &rox_library::rusqlite::Connection,
    retry_refused: bool,
) -> Result<Vec<store::BpmToMeasure>, String> {
    let missing = store::bpm_missing(conn, false).map_err(|e| e.to_string())?;
    if !retry_refused {
        return Ok(missing);
    }
    let untouched: std::collections::HashSet<i64> = missing.iter().map(|track| track.id).collect();
    let both = store::bpm_missing(conn, true).map_err(|e| e.to_string())?;
    Ok(both
        .into_iter()
        .filter(|track| !untouched.contains(&track.id))
        .collect())
}

/// Track-parallel through a bounded pool over a shared cursor. Workers only
/// share the database, locked once a batch builds up.
fn run(
    db_path: &Path,
    workers: usize,
    retry_refused: bool,
    progress: &Progress,
) -> Result<usize, String> {
    let conn = store::open(db_path).map_err(|e| e.to_string())?;
    let work = work_list(&conn, retry_refused)?;
    progress.total.store(work.len(), Ordering::Relaxed);
    progress.pace.begin();

    let conn = Mutex::new(conn);
    let written = AtomicUsize::new(0);
    // The first failed write ends the pass: the next batch would fail too.
    let failure: Mutex<Option<String>> = Mutex::new(None);
    let cursor = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(workers.max(1))
        .min(work.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                let mut batch: Vec<(String, u16, f32)> = Vec::with_capacity(BATCH);
                // Written as refusals so the next pass doesn't decode them again.
                let mut refused: Vec<(String, u16)> = Vec::with_capacity(BATCH);
                loop {
                    if !progress.keep_going() || failure.lock().unwrap().is_some() {
                        break;
                    }
                    let Some(track) = work.get(cursor.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    *progress.current.lock().unwrap() = track.path.clone();
                    // A cancel is honoured between tracks, not inside one.
                    match rox_acoustic::tempo::estimate(Path::new(&track.path), track.duration_ms) {
                        Ok(Some(bpm)) => batch.push((track.path.clone(), track.sub, bpm)),
                        // Measured, no tempo: the refused mark keeps it off the next list.
                        Ok(None) => {
                            log::debug!("tempo: no answer for {}", track.path);
                            progress.failed.fetch_add(1, Ordering::Relaxed);
                            refused.push((track.path.clone(), track.sub));
                        }
                        // Nothing decoded, so nothing to refuse: the row comes back next pass.
                        Err(rox_acoustic::tempo::Unreadable) => {
                            log::debug!("tempo: nothing decoded for {}", track.path);
                            progress.failed.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    progress.done.fetch_add(1, Ordering::Relaxed);
                    if (batch.len() >= BATCH || refused.len() >= BATCH)
                        && let Err(e) = flush(&mut batch, &mut refused, &conn, &written)
                    {
                        *failure.lock().unwrap() = Some(e);
                        return;
                    }
                }
                // Flush what's held on a stop too, so no finished decoding is lost.
                if let Err(e) = flush(&mut batch, &mut refused, &conn, &written) {
                    *failure.lock().unwrap() = Some(e);
                }
            });
        }
    });
    if let Some(e) = failure.into_inner().unwrap() {
        return Err(e);
    }
    Ok(written.into_inner())
}

/// Rows that picked up a tempo tag since the work list was built are
/// skipped, so the count is what actually took.
fn flush(
    batch: &mut Vec<(String, u16, f32)>,
    refused: &mut Vec<(String, u16)>,
    conn: &Mutex<rox_library::rusqlite::Connection>,
    written: &AtomicUsize,
) -> Result<(), String> {
    if !batch.is_empty() {
        let rows: Vec<(&str, u16, f32)> = batch
            .iter()
            .map(|(path, sub, bpm)| (path.as_str(), *sub, *bpm))
            .collect();
        let took =
            store::set_measured_bpm(&mut conn.lock().unwrap(), &rows).map_err(|e| e.to_string())?;
        written.fetch_add(took, Ordering::Relaxed);
        batch.clear();
    }
    if !refused.is_empty() {
        let rows: Vec<(&str, u16)> = refused
            .iter()
            .map(|(path, sub)| (path.as_str(), *sub))
            .collect();
        store::set_refused_bpm(&mut conn.lock().unwrap(), &rows).map_err(|e| e.to_string())?;
        refused.clear();
    }
    Ok(())
}
