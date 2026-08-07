//! The ReplayGain measurement pass (ADR 19): decode every file the library
//! has no gain for, meter it per EBU R128, and put the numbers somewhere the
//! player can read them.
//!
//! One pass at a time, app-global rather than owned by a window, so closing
//! the Audio page leaves it running and reopening it picks the progress back
//! up. The work itself is a blocking loop on the background executor over
//! [`rox_playback::analysis`]; the UI samples an `Arc<Progress>` on a timer,
//! the way the scan badge samples a scan.
//!
//! Where the numbers land follows the [`ReplayGainSave`] setting, read once
//! when the pass starts so a mid-run flip can't split one album across two
//! destinations.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use gpui::{App, Entity, Global};

use rox_library::rusqlite::Connection;
use rox_library::{replaygain, store, writer};
use rox_playback::analysis::{self, AlbumAnalysis};

use crate::catalog::Library;
use crate::settings::{ReplayGainSave, Settings};

/// Files a pass must get through before its rate is worth remembering as
/// this machine's pace, the acoustic pass's `PACE_FLOOR`'s twin.
const PACE_FLOOR: usize = 16;

/// Live progress of a measurement pass: the worker writes it per file, the
/// UI polls it. Zero total means the work list is still being built.
#[derive(Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    /// Files the analyzer could not read at all, so the readout can own up
    /// to a pass that skipped some.
    failed: AtomicUsize,
    /// Full path of a file being measured. Whichever worker wrote last, so
    /// it reads as a sample of the work rather than a queue position.
    current: Mutex<String>,
    /// Raised by [`stop`] and by app quit; the pass drops out at the next
    /// quarter second of audio.
    cancel: AtomicBool,
    /// The pass's clock, for the "about 2 hours left" half of the readout.
    /// Started once the work list is built, so the album walk doesn't bill
    /// the first file.
    pace: crate::pace::Pace,
}

impl Progress {
    /// Files measured or given up on so far.
    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    /// Files the pass set out to measure. Zero while the work list is still
    /// being built.
    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    /// Files that would not decode.
    pub fn failed(&self) -> usize {
        self.failed.load(Ordering::Relaxed)
    }

    /// The file under the cursor.
    pub fn current(&self) -> String {
        self.current.lock().unwrap().clone()
    }

    /// Whether a stop has been asked for and the pass is winding down.
    pub fn stopping(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Seconds each file has cost so far, measured over the whole pass.
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

/// Ask the running pass to stop at the next file. What it already wrote
/// stays; a no-op when nothing is running.
pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel.store(true, Ordering::Relaxed);
    }
}

/// Measure every library file with no ReplayGain and save what it measured.
/// A no-op while a pass is already running.
///
/// Safe to call from inside the library's own update: nothing reads the
/// entity until the spawned task, by which time the lease is gone. The
/// acoustic pass is started that way from the watch sync, and reading a
/// leased entity panics.
pub fn start(library: Entity<Library>, cx: &mut App) {
    if progress(cx).is_some() {
        return;
    }
    // Read once, here: the pass writes an album at a time, and a flip
    // halfway through would leave one record split between the database and
    // its own tags. The worker count is read here for the same reason, so a
    // pass keeps the pool it started with.
    let settings = Settings::load();
    let save = settings.replay_gain.save;
    let workers = settings.replaygain_workers.max(1);
    let progress = Arc::new(Progress::default());
    cx.set_global(Running(Some(progress.clone())));
    // Keeps the menubar chip and the tasks window ticking; nothing observes
    // an app-global pass on its own.
    crate::tasks_window::repaint_while_running(cx);
    // Quitting mid-pass shouldn't leave a tag write half done, so the same
    // flag the stop button raises goes up on the way out; the worker is
    // between files within a quarter second of audio.
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
                async move { run(&db_path, save, workers, &progress) }
            })
            .await;
        cx.update(|cx| {
            cx.set_global(Running(None));
            // What this machine measures per file, remembered so the next
            // Measure Missing can be priced before it runs. Worker-seconds,
            // like the acoustic pace, so the prompt can price any worker
            // count against it. Only off a decent stretch: a pass over a
            // handful of files measures its own startup, not the rate.
            if progress.done() >= PACE_FLOOR {
                if let Some(per) = progress.secs_per_track() {
                    let pace = (per * workers as f64) as f32;
                    Settings::update(move |s| s.session.replaygain_pace = pace);
                }
            }
            library.update(cx, |library, cx| match written {
                Ok(paths) => {
                    // Database mode wrote the columns itself and has nothing
                    // on disk to re-read, so it hands back an empty list and
                    // only the readouts refresh.
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

/// Time a few files to learn what this machine costs per file, so a first
/// pass can be priced before anyone commits an afternoon to it. Returns
/// worker-seconds per file, the unit [`crate::pace::estimate`] divides.
///
/// Nothing is written. Measurement is only sound over a whole album, and a
/// probe deliberately samples across the library rather than working through
/// one record, so what it measures isn't a shape that can be saved. In tags
/// mode saving would also mean rewriting audio files, which is not something
/// a button called Estimate should do. The cost is a few seconds of decoding
/// spent to avoid guessing at hours.
///
/// Rougher than the acoustic probe by nature: measuring reads the whole file,
/// so its cost follows duration, and three files can't know a library's
/// average length. It's the difference between "about 3 hours" and "about 5",
/// not between hours and days, which is the question being asked.
pub fn measure_pace(db_path: &Path) -> Result<f32, String> {
    let conn = store::open(db_path).map_err(|e| e.to_string())?;
    let albums = store::albums_missing_replaygain(&conn).map_err(|e| e.to_string())?;
    // Flattened back to files: albums vary from a single to a box set, so
    // sampling albums would let one long record stand for the library.
    let paths: Vec<&String> = albums.iter().flat_map(|a| &a.paths).collect();
    let picked = crate::pace::sample_indices(paths.len(), crate::pace::PROBE_TRACKS);
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

/// Whether this pass gets to put out an album gain, given how many of the
/// album's files it measured out of how many the album holds.
///
/// An album gain is the whole record gated as one program. Measure half the
/// tracks and the number you get is for a different record than the one on
/// disk, so a partial album gets track values only and its album columns are
/// left alone. Nothing is lost by that: the tracks that already carry tags
/// carry their album figures too, and those were measured over the real
/// thing.
///
/// A file with no album tag is `grouped: false` and never earns one however
/// alone it is. It's a file, not a record of one, and putting its own gain
/// in the album field would level it by itself next to a compilation.
fn measures_album(grouped: bool, measured: usize, album_total: usize) -> bool {
    grouped && measured > 0 && measured == album_total
}

/// The blocking half: walk the albums, measure, write. Returns the paths
/// whose files were rewritten, which is empty in database mode.
///
/// Album-parallel through a bounded pool, the acoustic pass's shape. The
/// album is the unit rather than the file because an album gain is measured
/// over the whole record: splitting one across workers would mean collecting
/// its tracks back together before anything could be written, and albums are
/// plentiful enough to keep every worker busy on their own.
///
/// The one thing workers share is the database, behind a mutex. That
/// serializes the writes, which is what SQLite wants anyway, and they're a
/// rounding error next to the decode either way.
fn run(
    db_path: &Path,
    save: ReplayGainSave,
    workers: usize,
    progress: &Progress,
) -> Result<Vec<PathBuf>, String> {
    let conn = store::open(db_path).map_err(|e| e.to_string())?;
    let albums = store::albums_missing_replaygain(&conn).map_err(|e| e.to_string())?;
    progress.total.store(
        albums.iter().map(|a| a.paths.len()).sum(),
        Ordering::Relaxed,
    );
    progress.pace.begin();

    let conn = Mutex::new(conn);
    let rewritten = Mutex::new(Vec::new());
    // The first write that failed, which ends the pass: a database that
    // won't take a row won't take the next one either, and grinding through
    // a library's worth of decoding to write none of it helps nobody.
    let failure: Mutex<Option<String>> = Mutex::new(None);
    let cursor = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(workers.max(1))
        .min(albums.len().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                if !progress.keep_going() || failure.lock().unwrap().is_some() {
                    break;
                }
                let Some(album) = albums.get(cursor.fetch_add(1, Ordering::Relaxed)) else {
                    break;
                };
                if let Err(e) = measure_album(album, save, &conn, &rewritten, progress) {
                    *failure.lock().unwrap() = Some(e);
                    break;
                }
            });
        }
    });
    if let Some(e) = failure.into_inner().unwrap() {
        return Err(e);
    }
    Ok(rewritten.into_inner().unwrap())
}

/// One album measured and written. Errors are the database's alone: a file
/// that won't decode and a tag write that won't land are both counted as
/// skipped and left behind, because the next album is unaffected by either.
fn measure_album(
    album: &store::AlbumToMeasure,
    save: ReplayGainSave,
    conn: &Mutex<rox_library::rusqlite::Connection>,
    rewritten: &Mutex<Vec<PathBuf>>,
    progress: &Progress,
) -> Result<(), String> {
    let mut program = AlbumAnalysis::new();
    let mut measured: Vec<String> = Vec::new();
    for path in &album.paths {
        if !progress.keep_going() {
            break;
        }
        *progress.current.lock().unwrap() = path.clone();
        // The per-file frame counts go unused: the readout counts files,
        // and a bar that jitters inside every track says less than one
        // that steps once per track.
        match analysis::measure(Path::new(path), || progress.keep_going(), |_, _| {}) {
            Ok(Some(track)) => {
                program.push(track);
                measured.push(path.clone());
            }
            // Cancelled mid-file; the album is incomplete either way.
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
    // Counted after the loop, not before it: a file that wouldn't decode
    // and a cancel partway both leave fewer files measured than the
    // album holds, and either one makes this the partial case.
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
            store::set_measured_replaygain(&mut conn.lock().unwrap(), &rows)
                .map_err(|e| e.to_string())?;
        }
        ReplayGainSave::Tags => {
            for (path, gain) in measured.iter().zip(gains) {
                let file = PathBuf::from(path);
                // commit_replay_gain clears any field it's handed None,
                // which is right for a re-measure and wrong here: the
                // partial case leaves the album pair empty on purpose,
                // and a file that already carried album numbers from a
                // tagger would lose them. Carry the row's through.
                //
                // The lock is held for the read alone: the tag write is the
                // slow half and touches only this file, so every other
                // worker is free to reach the database while it runs.
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

/// The engine's ReplayGain as the library spells it. Same four numbers, two
/// structs, because neither crate depends on the other.
fn bridge(gain: rox_playback::gain::ReplayGain) -> replaygain::ReplayGain {
    replaygain::ReplayGain {
        track_db: gain.track_db,
        track_peak: gain.track_peak,
        album_db: gain.album_db,
        album_peak: gain.album_peak,
    }
}

/// Fill a measurement's empty album fields from what the row already holds,
/// so a tag write only ever adds. A row we can't read leaves them empty,
/// which is the same answer as a row that never had them.
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

/// The measurement's own album figures where it has them, the row's where it
/// doesn't. Never the other way round: what this pass measured over a whole
/// record beats whatever a tagger left behind.
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

    /// A whole album measured together earns an album gain; anything less
    /// does not, however close it gets.
    #[test]
    fn only_a_whole_album_earns_an_album_gain() {
        assert!(measures_album(true, 12, 12));
        // A single-track record is still a record.
        assert!(measures_album(true, 1, 1));
        assert!(!measures_album(true, 11, 12));
        // Nothing measured is not a whole album, whatever the total says.
        assert!(!measures_album(true, 0, 0));
        assert!(!measures_album(true, 0, 5));
        // A file with no album tag is its own unit and never gets one.
        assert!(!measures_album(false, 1, 1));
    }

    /// A partial album's track numbers go in without disturbing whatever
    /// album figures the file already carried, which is what keeps a tag
    /// write from being a deletion.
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
        // The track pair is this pass's, untouched by the old row.
        assert_eq!(merged.track_db, Some(-6.5));
        assert_eq!(merged.track_peak, Some(0.97));
        assert_eq!(merged.album_db, Some(-8.1));
        assert_eq!(merged.album_peak, Some(0.99));
    }

    /// A whole album measured here keeps its own figures; the row's older
    /// pair does not get a vote.
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
