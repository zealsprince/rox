//! The ReplayGain measurement pass (ADR 19): decode every file the library has
//! no gain for, meter it per EBU R128, and save the numbers per the
//! [`ReplayGainSave`] setting. App-global, so it outlives the window that
//! started it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{App, Entity, Global};

use rox_library::rusqlite::Connection;
use rox_library::{replaygain, store, writer};
use rox_playback::analysis::{self, AlbumAnalysis};

use rox_core::settings::{ReplayGainSave, Settings};
use rox_services::catalog::{Library, LibraryJob};

/// Files a pass must finish before its rate counts as this machine's pace.
const PACE_FLOOR: usize = 16;

#[derive(Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    failed: AtomicUsize,
    current: Mutex<String>,
    /// Raised by [`stop`] and by app quit; the pass drops out within a quarter
    /// second of audio.
    cancel: AtomicBool,
    /// Started once the work list is built, so the album query doesn't bill the
    /// first file.
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

/// A no-op while a pass is already running. Safe to call inside the library's
/// own update: nothing reads the entity until the spawned task.
pub fn start(library: Entity<Library>, cx: &mut App) {
    if progress(cx).is_some() {
        return;
    }
    // Read once: a mid-run flip would split one album between the database and
    // its tags.
    let settings = Settings::load();
    let save = settings.replay_gain.save;
    let workers = settings.replaygain_workers.max(1);
    let progress = Arc::new(Progress::default());
    cx.set_global(Running(Some(progress.clone())));
    crate::tasks_window::repaint_while_running(cx);
    // Quitting mid-pass raises the stop flag, so no tag write is left half
    // done.
    cx.on_app_quit({
        let progress = progress.clone();
        move |_| {
            progress.cancel.store(true, Ordering::Relaxed);
            async {}
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        let Ok(db_path) = cx.update(|cx| library.read(cx).db_path()) else {
            return;
        };
        let written = cx
            .background_executor()
            .spawn({
                let progress = progress.clone();
                async move { run(&db_path, save, workers, &progress) }
            })
            .await;
        cx.update(|cx| {
            cx.set_global(Running(None));
            // Worker-seconds, so the prompt can price any worker count. Only
            // off a decent stretch: a short pass measures its own startup.
            if progress.done() >= PACE_FLOOR
                && let Some(per) = progress.secs_per_track()
            {
                let pace = (per * workers as f64) as f32;
                Settings::update(move |s| s.session.replaygain_pace = pace);
            }
            library.update(cx, |library, cx| match written {
                // Database mode only moved rows the projection packs, so the
                // cheap reload is the whole refresh.
                Ok((_, stored)) if stored > 0 => library.reload_projection(cx),
                Ok((paths, _)) => {
                    library.reindex_written(paths, cx);
                }
                Err(e) => {
                    log::error!("replaygain: {e}");
                    library.note_gain_written(cx);
                }
            });
        })
        .ok();
    })
    .detach();
}

/// Only the watcher's delta: the backlog is priced and agreed to when the
/// switch goes on. The switch is checked here, not in [`start`], so the button
/// works with it off.
pub fn follow(library: &Entity<Library>, cx: &mut App) {
    App::subscribe(cx, library, |library, event, cx| {
        if matches!(event, LibraryJob::WatchSettled) && Settings::load().replay_gain.auto {
            start(library, cx);
        }
    })
    .detach();
}

/// Time a few files so a first pass can be priced. Returns worker-seconds per
/// file.
///
/// Nothing is written: measurement is only sound over a whole album, and in
/// tags mode saving would rewrite audio files. Rough, since cost follows
/// duration and three files can't know the library's average length.
pub fn measure_pace(db_path: &Path) -> Result<f32, String> {
    let conn = store::open(db_path).map_err(|e| e.to_string())?;
    let albums = store::albums_missing_replaygain(&conn).map_err(|e| e.to_string())?;
    // Sampled per file, not per album, so one box set can't stand for the
    // library.
    let paths: Vec<&String> = albums.iter().flat_map(|a| &a.paths).collect();
    let picked = rox_core::pace::sample_indices(paths.len(), rox_core::pace::PROBE_TRACKS);
    if picked.is_empty() {
        return Err("there's nothing left to measure".into());
    }

    let started = Instant::now();
    let mut measured = 0usize;
    let mut last_err = String::new();
    for index in picked {
        let path = paths[index];
        match analysis::measure(Path::new(path), || true, |_, _| {}) {
            Ok(Some(_)) => measured += 1,
            Ok(None) => {}
            Err(e) => {
                log::warn!("replaygain: probing {path}: {e}");
                last_err = e;
            }
        }
    }
    if measured == 0 {
        return Err(if last_err.is_empty() {
            "nothing decodable".into()
        } else {
            last_err
        });
    }
    Ok((started.elapsed().as_secs_f64() / measured as f64) as f32)
}

/// An album gain is the whole record gated as one program, so a partial album
/// gets track values only. A file with no album tag never earns one.
fn measures_album(grouped: bool, measured: usize, album_total: usize) -> bool {
    grouped && measured > 0 && measured == album_total
}

/// Returns the rewritten paths (tags mode) and the rows that took a gain
/// (database mode).
///
/// Album-parallel, since an album gain needs the whole record in one worker.
/// Workers share only the database, behind a mutex.
fn run(
    db_path: &Path,
    save: ReplayGainSave,
    workers: usize,
    progress: &Progress,
) -> Result<(Vec<PathBuf>, usize), String> {
    let conn = store::open(db_path).map_err(|e| e.to_string())?;
    let albums = store::albums_missing_replaygain(&conn).map_err(|e| e.to_string())?;
    progress.total.store(
        albums.iter().map(|a| a.paths.len()).sum(),
        Ordering::Relaxed,
    );
    progress.pace.begin();

    let conn = Mutex::new(conn);
    let rewritten = Mutex::new(Vec::new());
    // Counted so a pass that stored nothing skips the projection reload; the
    // auto pass runs off every watch settle.
    let stored = AtomicUsize::new(0);
    // The first failed write ends the pass: the database won't take the next
    // row either.
    let failure: Mutex<Option<String>> = Mutex::new(None);
    let cursor = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(workers.max(1))
        .min(albums.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    if !progress.keep_going() || failure.lock().unwrap().is_some() {
                        break;
                    }
                    let Some(album) = albums.get(cursor.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    if let Err(e) = measure_album(album, save, &conn, &rewritten, &stored, progress)
                    {
                        *failure.lock().unwrap() = Some(e);
                        break;
                    }
                }
            });
        }
    });
    if let Some(e) = failure.into_inner().unwrap() {
        return Err(e);
    }
    Ok((rewritten.into_inner().unwrap(), stored.into_inner()))
}

/// Only database errors propagate; a file that won't decode or a failed tag
/// write counts as skipped.
fn measure_album(
    album: &store::AlbumToMeasure,
    save: ReplayGainSave,
    conn: &Mutex<rox_library::rusqlite::Connection>,
    rewritten: &Mutex<Vec<PathBuf>>,
    stored: &AtomicUsize,
    progress: &Progress,
) -> Result<(), String> {
    let mut program = AlbumAnalysis::new();
    let mut measured: Vec<String> = Vec::new();
    for path in &album.paths {
        if !progress.keep_going() {
            break;
        }
        *progress.current.lock().unwrap() = path.clone();
        // The per-file frame counts go unused: the readout steps once per file.
        match analysis::measure(Path::new(path), || progress.keep_going(), |_, _| {}) {
            Ok(Some(track)) => {
                program.push(track);
                measured.push(path.clone());
            }
            Ok(None) => break,
            Err(e) => {
                log::warn!("replaygain: {path}: {e}");
                progress.failed.fetch_add(1, Ordering::Relaxed);
            }
        }
        progress.done.fetch_add(1, Ordering::Relaxed);
    }
    if measured.is_empty() {
        return Ok(());
    }
    // After the loop: a decode failure or a cancel both make this the partial
    // case.
    let whole = measures_album(album.group.is_some(), measured.len(), album.total);
    let gains: Vec<replaygain::ReplayGain> = if whole {
        program.replay_gains().into_iter().map(bridge).collect()
    } else {
        program
            .tracks()
            .iter()
            .map(|t| bridge(t.replay_gain()))
            .collect()
    };
    match save {
        ReplayGainSave::Database => {
            let rows: Vec<(&str, replaygain::ReplayGain)> = measured
                .iter()
                .map(String::as_str)
                .zip(gains.iter().copied())
                .collect();
            let took = store::set_measured_replaygain(&mut conn.lock().unwrap(), &rows)
                .map_err(|e| e.to_string())?;
            stored.fetch_add(took, Ordering::Relaxed);
        }
        ReplayGainSave::Tags => {
            for (path, gain) in measured.iter().zip(gains) {
                let file = PathBuf::from(path);
                // `commit_replay_gain` clears any field handed None, so copy
                // the row's album pair through or a partial album would erase a
                // tagger's numbers. The lock covers the read only, not the slow
                // tag write.
                let gain = fill_album(&conn.lock().unwrap(), path, gain);
                match writer::commit_replay_gain(&file, gain) {
                    Ok(()) => rewritten.lock().unwrap().push(file),
                    Err(e) => {
                        log::warn!("replaygain: writing {path}: {e}");
                        progress.failed.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
    }
    Ok(())
}

fn bridge(gain: rox_playback::gain::ReplayGain) -> replaygain::ReplayGain {
    replaygain::ReplayGain {
        track_db: gain.track_db,
        track_peak: gain.track_peak,
        album_db: gain.album_db,
        album_peak: gain.album_peak,
    }
}

/// So a tag write only ever adds.
fn fill_album(
    conn: &Connection,
    path: &str,
    gain: replaygain::ReplayGain,
) -> replaygain::ReplayGain {
    if gain.album_db.is_some() && gain.album_peak.is_some() {
        return gain;
    }
    match store::queue_meta_for_path(conn, path) {
        Ok(meta) => merge_album(gain, meta.replay_gain),
        Err(_) => gain,
    }
}

/// A measured album pair always beats a tagger's.
fn merge_album(
    gain: replaygain::ReplayGain,
    existing: replaygain::ReplayGain,
) -> replaygain::ReplayGain {
    replaygain::ReplayGain {
        album_db: gain.album_db.or(existing.album_db),
        album_peak: gain.album_peak.or(existing.album_peak),
        ..gain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_whole_album_earns_an_album_gain() {
        assert!(measures_album(true, 12, 12));
        assert!(measures_album(true, 1, 1));
        assert!(!measures_album(true, 11, 12));
        assert!(!measures_album(true, 0, 0));
        assert!(!measures_album(true, 0, 5));
        assert!(!measures_album(false, 1, 1));
    }

    #[test]
    fn a_blank_album_pair_falls_back_to_the_row() {
        let existing = replaygain::ReplayGain {
            track_db: Some(-9.9),
            track_peak: Some(0.5),
            album_db: Some(-8.1),
            album_peak: Some(0.99),
        };
        let partial = replaygain::ReplayGain {
            track_db: Some(-6.5),
            track_peak: Some(0.97),
            ..Default::default()
        };
        let merged = merge_album(partial, existing);
        assert_eq!(merged.track_db, Some(-6.5));
        assert_eq!(merged.track_peak, Some(0.97));
        assert_eq!(merged.album_db, Some(-8.1));
        assert_eq!(merged.album_peak, Some(0.99));
    }

    #[test]
    fn a_measured_album_pair_wins_over_the_row() {
        let existing = replaygain::ReplayGain {
            album_db: Some(-8.1),
            album_peak: Some(0.99),
            ..Default::default()
        };
        let whole = replaygain::ReplayGain {
            track_db: Some(-6.5),
            track_peak: Some(0.97),
            album_db: Some(-7.0),
            album_peak: Some(1.01),
        };
        assert_eq!(merge_album(whole, existing), whole);
    }
}
