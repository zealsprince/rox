//! The acoustic analysis pass, as the app sees it.
//!
//! Both extractors, the model catalog, and the pass itself are defined in
//! [`rox_acoustic`], which took candle with it. This module holds the
//! app-global bookkeeping around a running pass: the `Arc<Progress>` the
//! tasks window and the settings page sample on a timer, the failure the
//! last pass left behind, and the spawn that keeps the blocking half off the
//! main thread. Everything else goes through [`rox_acoustic`] directly.
//!
//! The shape is the ReplayGain measurement's ([`crate::replaygain_job`]):
//! app-global rather than owned by a window, blocking work on the background
//! executor, progress polled rather than pushed.

pub mod models;

use std::sync::Arc;

use gpui::{App, Entity, Global};

use rox_acoustic::Progress;
use rox_core::settings::Settings;
use rox_services::catalog::{Library, LibraryJob};

/// The running pass, or nothing. App-global so it outlives the settings
/// window that started it.
#[derive(Default)]
struct Running(Option<Arc<Progress>>);

impl Global for Running {}

/// The last pass's failure, kept after the pass is gone so the settings
/// page can still explain why nothing happened. A model whose weights won't
/// load is the case this exists for: without it the button would flash and
/// the coverage line would be unchanged, with the reason only in the log.
#[derive(Default)]
struct LastFailure(Option<String>);

impl Global for LastFailure {}

/// The running pass's progress, for any UI that shows it. None when
/// nothing is analyzing.
pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

/// Why the last pass stopped early, if it did.
pub fn last_failure(cx: &App) -> Option<String> {
    cx.try_global::<LastFailure>().and_then(|f| f.0.clone())
}

/// Signal the running pass to stop at the next file. What it already wrote
/// stays; a no-op when nothing is running.
pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel();
    }
}

/// Analyze every track with no vector for the selected model. A no-op while
/// a pass is already running, and while the feature is switched off.
///
/// Which model runs is [`rox_services::acoustic::acoustic_source`], resolved here
/// rather than passed in: it's the same pick the similarity queries read, and
/// a caller that could hand in a different one would be able to fill the
/// table under a name nothing reads.
///
/// The library entity comes in rather than its database path, which it used
/// to and which was the lighter thing to pass. In tags mode the pass writes
/// audio files, and rox watches the folders those files are in, so every
/// write comes straight back through the watcher as a change to reindex
/// unless the library is told the writes were its own. That's
/// [`Library::reindex_written`], and calling it needs the entity. Database
/// mode returns an empty list and only the readouts refresh.
///
/// Safe to call from inside the library's own update, which the watch sync
/// does: the entity isn't read until the spawned task, by which time the
/// lease is gone. Reading a leased entity panics, so the `db_path` read
/// happens down there rather than up here.
pub fn start(library: Entity<Library>, cx: &mut App) {
    let settings = Settings::load();
    if progress(cx).is_some() || !settings.acoustic_analysis {
        return;
    }
    // Read once here rather than inside the pass: a pass keeps the worker
    // count it started with, and the next one picks up a changed setting.
    // Where the vectors go is read the same way, so a mid-pass flip can't
    // leave one album's tracks split between two destinations.
    let workers = settings.acoustic_workers.max(1);
    let save = settings.acoustic_save;
    let source = rox_services::acoustic::acoustic_source();
    let progress = Arc::new(Progress::new(source.id()));
    cx.set_global(Running(Some(progress.clone())));
    cx.set_global(LastFailure(None));
    // Keeps the menubar chip and the tasks window ticking; nothing observes
    // an app-global pass on its own.
    crate::tasks_window::repaint_while_running(cx);
    // Quitting mid-pass raises the same flag the stop button does, so the
    // workers stop on a batch boundary instead of being killed mid-write.
    cx.on_app_quit({
        let progress = progress.clone();
        move |_| {
            progress.cancel();
            async {}
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        let name = source.id().to_string();
        // The library holds its own database path, and reading it here
        // rather than up top keeps a caller inside its update safe. The
        // read only fails with the app already on its way out.
        let Ok(db_path) = cx.update(|cx| library.read(cx).db_path()) else {
            return;
        };
        let result = cx
            .background_executor()
            .spawn({
                let progress = progress.clone();
                async move { rox_acoustic::run(&source, &db_path, workers, save, &progress) }
            })
            .await;
        cx.update(|cx| {
            cx.set_global(Running(None));
            // What this machine can do, remembered for the next estimate.
            // Worker-seconds per track, so the Library page can price any
            // worker setting against it. Only off a decent stretch: a pass
            // over a handful of files measures its own startup, not the rate.
            if progress.done() >= rox_acoustic::PACE_FLOOR {
                if let Some(per) = progress.secs_per_track() {
                    let pace = (per * workers as f64) as f32;
                    let id = name.clone();
                    Settings::update(move |s| {
                        s.session.acoustic_pace.insert(id, pace);
                    });
                }
            }
            match result {
                Ok(analyzed) => {
                    // The surfaces that offer ordering by sound are gated on
                    // there being vectors, and this is the moment there are.
                    if analyzed.described > 0 {
                        rox_core::settings::set_acoustic_described(true, cx);
                    }
                    log::info!(
                        "acoustic: {} tracks analyzed with {name}, {} tagged",
                        analyzed.described,
                        analyzed.tagged.len()
                    );
                    // The files this pass rewrote, claimed as the app's own
                    // before the watcher brings them back as changes.
                    library.update(cx, |library, cx| {
                        library.reindex_written(analyzed.tagged, cx)
                    });
                }
                Err(e) => {
                    log::error!("acoustic: {e}");
                    cx.set_global(LastFailure(Some(e)));
                }
            }
        })
        .ok();
    })
    .detach();
}

/// Follow a library's watch syncs, so a library with the auto switch on
/// stays described as it grows instead of waiting for someone to open the
/// settings and press a button.
///
/// The switch is read here rather than inside [`start`], the ReplayGain
/// follow's stance: the button has to keep working with the switch off, and
/// this is the only caller the setting applies to. Off by default, so
/// turning analysis on doesn't also start a pass on every watch settle.
///
/// Only what the watcher brought in. A full scan is an import
/// or a manual rescan, and a library's worth of decoding is an afternoon that
/// should be asked for; the catalog draws that line and only emits for the
/// watch case.
pub fn follow(library: &Entity<Library>, cx: &mut App) {
    App::subscribe(cx, library, |library, event, cx| {
        if matches!(event, LibraryJob::WatchSettled) && Settings::load().acoustic_auto {
            start(library, cx);
        }
    })
    .detach();
}
