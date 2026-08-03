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

use gpui::{App, Entity, Global};

use rox_library::rusqlite::Connection;
use rox_library::{replaygain, store, writer};
use rox_playback::analysis::{self, AlbumAnalysis};

use crate::catalog::Library;
use crate::settings::{ReplayGainSave, Settings};

/// Live progress of a measurement pass: the worker writes it per file, the
/// UI polls it. Zero total means the work list is still being built.
#[derive(Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    /// Files the analyzer could not read at all, so the readout can own up
    /// to a pass that skipped some.
    failed: AtomicUsize,
    /// Full path of the file being measured.
    current: Mutex<String>,
    /// Raised by [`stop`] and by app quit; the pass drops out at the next
    /// quarter second of audio.
    cancel: AtomicBool,
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
pub fn start(library: Entity<Library>, cx: &mut App) {
    if progress(cx).is_some() {
        return;
    }
    let db_path = library.read(cx).db_path();
    // Read once, here: the pass writes an album at a time, and a flip
    // halfway through would leave one record split between the database and
    // its own tags.
    let save = Settings::load().replay_gain.save;
    let progress = Arc::new(Progress::default());
    cx.set_global(Running(Some(progress.clone())));
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
        let written = cx
            .background_executor()
            .spawn({
                let progress = progress.clone();
                async move { run(&db_path, save, &progress) }
            })
            .await;
        cx.update(|cx| {
            cx.set_global(Running(None));
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
/// Sequential on purpose for the first cut, one album and one file at a
/// time. Album-parallel is the obvious upgrade and the work is already
/// grouped for it: each album is independent, so a worker pool over the
/// album list would scale straight across cores with no change to what gets
/// written.
fn run(db_path: &Path, save: ReplayGainSave, progress: &Progress) -> Result<Vec<PathBuf>, String> {
    let mut conn = store::open(db_path).map_err(|e| e.to_string())?;
    let albums = store::albums_missing_replaygain(&conn).map_err(|e| e.to_string())?;
    progress.total.store(
        albums.iter().map(|a| a.paths.len()).sum(),
        Ordering::Relaxed,
    );

    let mut rewritten = Vec::new();
    for album in albums {
        if !progress.keep_going() {
            break;
        }
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
            continue;
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
                store::set_measured_replaygain(&mut conn, &rows).map_err(|e| e.to_string())?;
            }
            ReplayGainSave::Tags => {
                for (path, gain) in measured.iter().zip(gains) {
                    let file = PathBuf::from(path);
                    // commit_replay_gain clears any field it's handed None,
                    // which is right for a re-measure and wrong here: the
                    // partial case leaves the album pair empty on purpose,
                    // and a file that already carried album numbers from a
                    // tagger would lose them. Carry the row's through.
                    let gain = fill_album(&conn, path, gain);
                    match writer::commit_replay_gain(&file, gain) {
                        Ok(()) => rewritten.push(file),
                        Err(e) => {
                            log::warn!("replaygain: writing {path}: {e}");
                            progress.failed.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
    }
    Ok(rewritten)
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
