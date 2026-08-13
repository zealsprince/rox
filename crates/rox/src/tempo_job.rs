//! The tempo measurement pass: work out what every track with no BPM runs
//! at, and put the number on its row.
//!
//! The estimator lives in [`rox_acoustic::tempo`] and the work list and the
//! write in [`rox_library::store`]; what's here is the app-global
//! bookkeeping around a running pass, the shape [`crate::replaygain_job`]
//! and [`crate::embeddings`] already have. One pass at a time, owned by the
//! app rather than a window, so closing the Library page leaves it running
//! and reopening it picks the count back up.
//!
//! The database is the only place a tempo lands. Unlike the other two
//! passes there's no tags mode to choose: writing TBPM back into the audio
//! files would mean rewriting them to record an estimate the file's own
//! tagger never made, and a cue subsong shares its file with every other
//! track on the disc, which has nowhere to put a per-track number.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{App, Entity, Global};

use rox_library::store;

use rox_core::settings::Settings;
use rox_services::catalog::{Library, LibraryJob};

/// Tracks a pass must get through before its rate is worth remembering as
/// this machine's pace, the other two passes' `PACE_FLOOR`'s twin.
const PACE_FLOOR: usize = 16;

/// How many measured tempos one transaction carries. A tempo is a fraction
/// of a row and the estimate before it is a minute of decoding, so the
/// batch is about not holding a write lock per track rather than about the
/// writes costing anything. Small enough that a pass stopped halfway has
/// kept nearly everything it measured.
const BATCH: usize = 32;

/// Live progress of a tempo pass: the workers write it per track, the UI
/// polls it. Zero total means the work list is still being built.
#[derive(Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    /// Tracks the pass measured and got no straight answer for, so the
    /// readout can own up to a pass that wrote less than it looked at. A
    /// file that wouldn't decode and one whose beat the estimator refused
    /// to call are both in here: neither has a tempo to store, and the
    /// difference between them is in the log.
    failed: AtomicUsize,
    /// Full path of a track being measured. Whichever worker wrote last, so
    /// it reads as a sample of the work rather than a queue position.
    current: Mutex<String>,
    /// Raised by [`stop`] and by app quit; the workers drop out at the next
    /// track.
    cancel: AtomicBool,
    /// The pass's clock, for the "about 2 hours left" half of the readout.
    /// Started once the work list is built, so the query doesn't bill the
    /// first track.
    pace: rox_core::pace::Pace,
}

impl Progress {
    /// Tracks measured or given up on so far.
    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    /// Tracks the pass set out to measure. Zero while the work list is
    /// still being built.
    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    /// Tracks that came back without a tempo.
    pub fn failed(&self) -> usize {
        self.failed.load(Ordering::Relaxed)
    }

    /// The track under the cursor.
    pub fn current(&self) -> String {
        self.current.lock().unwrap().clone()
    }

    /// Whether a stop has been asked for and the pass is winding down.
    pub fn stopping(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Seconds each track has cost so far, measured over the whole pass.
    /// None until enough have finished for the average to mean anything.
    pub fn secs_per_track(&self) -> Option<f64> {
        self.pace.secs_per_track(self.done())
    }

    /// Seconds the rest of the pass should take at the rate so far.
    pub fn eta_secs(&self) -> Option<f64> {
        self.pace.eta_secs(self.done(), self.total())
    }

    fn keep_going(&self) -> bool {
        !self.cancel.load(Ordering::Relaxed)
    }
}

/// The running pass, or nothing. App-global so the pass outlives the window
/// that started it.
#[derive(Default)]
struct Running(Option<Arc<Progress>>);

impl Global for Running {}

/// The running pass's progress, for a UI that wants to show it. None when
/// nothing is measuring.
pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

/// Ask the running pass to stop at the next track. What it already wrote
/// stays; a no-op when nothing is running.
pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel.store(true, Ordering::Relaxed);
    }
}

/// Measure every library track with no tempo and save what came back. A
/// no-op while a pass is already running, and while the feature is switched
/// off.
///
/// The switch is read here rather than only at the caller, the acoustic
/// pass's arrangement: the toggle is the feature as much as the permission,
/// so with it off there's no button to keep working either.
///
/// Safe to call from inside the library's own update, which the watch sync
/// does: nothing reads the entity until the spawned task, by which time the
/// lease is gone, and reading a leased entity panics.
pub fn start(library: Entity<Library>, cx: &mut App) {
    let settings = Settings::load();
    if progress(cx).is_some() || !settings.tempo_analysis {
        return;
    }
    // Read once, here: a pass keeps the pool it started with, and the next
    // one picks up a changed setting.
    let workers = settings.tempo_workers.max(1);
    let progress = Arc::new(Progress::default());
    cx.set_global(Running(Some(progress.clone())));
    // Keeps the menubar chip and the tasks window ticking; nothing observes
    // an app-global pass on its own.
    crate::tasks_window::repaint_while_running(cx);
    // Quitting mid-pass raises the same flag the stop button does, so the
    // workers land on a batch boundary instead of being killed mid-write.
    cx.on_app_quit({
        let progress = progress.clone();
        move |_| {
            progress.cancel.store(true, Ordering::Relaxed);
            async {}
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        // The library says where its database is, and asking here rather
        // than up top is what keeps a caller inside its update safe. The
        // read only fails with the app already on its way out, where the
        // flag raised above has nothing left to mislead.
        let Ok(db_path) = cx.update(|cx| library.read(cx).db_path()) else {
            return;
        };
        let written = cx
            .background_executor()
            .spawn({
                let progress = progress.clone();
                async move { run(&db_path, workers, &progress) }
            })
            .await;
        cx.update(|cx| {
            cx.set_global(Running(None));
            // What this machine measures per track, remembered so the next
            // Analyze Missing can be priced before it runs. Worker-seconds,
            // like the other two paces, so the prompt can price any worker
            // count against it. Only off a decent stretch: a pass over a
            // handful of tracks measures its own startup, not the rate.
            if progress.done() >= PACE_FLOOR {
                if let Some(per) = progress.secs_per_track() {
                    let pace = (per * workers as f64) as f32;
                    Settings::update(move |s| s.session.tempo_pace = pace);
                }
            }
            match written {
                Ok(0) => {}
                Ok(written) => {
                    log::info!("tempo: {written} tracks measured");
                    // The tempos went straight onto rows the projection
                    // holds a packed copy of, and nothing on disk moved, so
                    // the cheap reload is the whole refresh: without it the
                    // BPM column keeps drawing the blanks the pass just
                    // filled in.
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

/// Follow a library's watch syncs, so a library with the switch on keeps
/// its tempos as it grows instead of waiting for someone to open the
/// settings and press a button.
///
/// Only what the watcher brought in, deliberately. The backlog a library
/// starts with is a decision made in front of an estimate, which is what
/// the start prompt is for; every settle after that only ever sees the
/// delta, which is the case this exists for.
pub fn follow(library: &Entity<Library>, cx: &mut App) {
    App::subscribe(cx, library, |library, event, cx| {
        if matches!(event, LibraryJob::WatchSettled) {
            start(library, cx);
        }
    })
    .detach();
}

/// Time a few tracks to learn what this machine costs per track, so a first
/// pass can be priced before anyone commits an afternoon to it. Returns
/// worker-seconds per track, the unit [`rox_core::pace::estimate`] divides.
///
/// Nothing is written, the ReplayGain probe's stance: an estimate is cheap
/// to redo, and a button called Estimate that quietly filled in three rows
/// would be doing work nobody asked it for. What it costs is a couple of
/// minutes of decoding to avoid guessing at hours.
pub fn measure_pace(db_path: &Path) -> Result<f32, String> {
    let conn = store::open(db_path).map_err(|e| e.to_string())?;
    let work = store::bpm_missing(&conn).map_err(|e| e.to_string())?;
    let picked = rox_core::pace::sample_indices(work.len(), rox_core::pace::PROBE_TRACKS);
    if picked.is_empty() {
        return Err("there's nothing left to measure".into());
    }

    let started = Instant::now();
    let mut timed = 0usize;
    for index in picked {
        let track = &work[index];
        // A track the estimator refuses still cost its decode, which is
        // what's being timed, so it counts the same as one that answered.
        rox_acoustic::tempo::estimate(Path::new(&track.path), track.duration_ms);
        timed += 1;
    }
    Ok((started.elapsed().as_secs_f64() / timed as f64) as f32)
}

/// The blocking half: walk the work list, estimate, write. Returns how many
/// rows took a tempo.
///
/// Track-parallel through a bounded pool over a shared cursor. The track is
/// the unit because a tempo is one track's own property, unlike a
/// ReplayGain album, so there's nothing to collect back together and a
/// worker can write whatever it has whenever it has it.
///
/// The one thing workers share is the database, behind a mutex, and they
/// only reach for it once a batch has built up. That serializes the writes,
/// which is what SQLite wants anyway, and they're a rounding error next to
/// the decoding either way.
fn run(db_path: &Path, workers: usize, progress: &Progress) -> Result<usize, String> {
    let conn = store::open(db_path).map_err(|e| e.to_string())?;
    let work = store::bpm_missing(&conn).map_err(|e| e.to_string())?;
    progress.total.store(work.len(), Ordering::Relaxed);
    progress.pace.begin();

    let conn = Mutex::new(conn);
    let written = AtomicUsize::new(0);
    // The first write that failed, which ends the pass: a database that
    // won't take a batch won't take the next one either, and grinding
    // through a library's worth of decoding to write none of it helps
    // nobody.
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
                // Held per worker rather than shared, so the only thing
                // two workers ever contend for is the write itself.
                let mut batch: Vec<(String, u16, f32)> = Vec::with_capacity(BATCH);
                loop {
                    if !progress.keep_going() || failure.lock().unwrap().is_some() {
                        break;
                    }
                    let Some(track) = work.get(cursor.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    *progress.current.lock().unwrap() = track.path.clone();
                    // The estimator reads its own two windows and answers
                    // in a couple of seconds, so a cancel is honoured
                    // between tracks rather than inside one.
                    match rox_acoustic::tempo::estimate(Path::new(&track.path), track.duration_ms) {
                        Some(bpm) => batch.push((track.path.clone(), track.sub, bpm)),
                        // Measured and refused. The row keeps its NULL,
                        // which is how a track nobody has looked at and one
                        // whose beat can't be called stay the same thing to
                        // the store and different things in the log.
                        None => {
                            log::debug!("tempo: no answer for {}", track.path);
                            progress.failed.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    progress.done.fetch_add(1, Ordering::Relaxed);
                    if batch.len() >= BATCH {
                        if let Err(e) = flush(&mut batch, &conn, &written) {
                            *failure.lock().unwrap() = Some(e);
                            return;
                        }
                    }
                }
                // Whatever the worker was still holding when it ran out of
                // work or was stopped: a cancel shouldn't throw away
                // minutes of decoding that already has its answer.
                if let Err(e) = flush(&mut batch, &conn, &written) {
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

/// One batch onto its rows, in a single transaction, leaving the batch
/// empty. Rows that picked up a tempo tag since the work list was built are
/// skipped by the store, so the count is what actually took.
fn flush(
    batch: &mut Vec<(String, u16, f32)>,
    conn: &Mutex<rox_library::rusqlite::Connection>,
    written: &AtomicUsize,
) -> Result<(), String> {
    if batch.is_empty() {
        return Ok(());
    }
    let rows: Vec<(&str, u16, f32)> = batch
        .iter()
        .map(|(path, sub, bpm)| (path.as_str(), *sub, *bpm))
        .collect();
    let took =
        store::set_measured_bpm(&mut conn.lock().unwrap(), &rows).map_err(|e| e.to_string())?;
    written.fetch_add(took, Ordering::Relaxed);
    batch.clear();
    Ok(())
}
