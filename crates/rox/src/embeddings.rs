//! The acoustic analysis pass, as the app sees it.
//!
//! Both extractors, the model catalog, and the pass itself live in
//! [`rox_acoustic`], which took candle with it. What's left here is the
//! app-global bookkeeping around a running pass: the `Arc<Progress>` the
//! tasks window and the settings page sample on a timer, the failure the
//! last pass left behind, and the spawn that keeps the blocking half off the
//! main thread. Everything else is re-exported below, so every
//! `crate::embeddings::` path in the app reads the way it always did.
//!
//! The shape is the ReplayGain measurement's ([`crate::replaygain_job`]):
//! app-global rather than owned by a window, blocking work on the background
//! executor, progress polled rather than pushed.

pub mod models;

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, Global};

use crate::settings::Settings;

// Glob rather than a list: the shim's job is that every name the app already
// pathed through here still resolves, and a list would quietly drop the ones
// nothing happens to reference today. The `models` module declared above
// shadows the one this brings in, which is the point of it.
pub use rox_acoustic::*;

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

/// The running pass's progress, for a UI that wants to show it. None when
/// nothing is analyzing.
pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

/// Why the last pass stopped early, if it did.
pub fn last_failure(cx: &App) -> Option<String> {
    cx.try_global::<LastFailure>().and_then(|f| f.0.clone())
}

/// Ask the running pass to stop at the next file. What it already wrote
/// stays; a no-op when nothing is running.
pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel();
    }
}

/// Analyze every track with no vector for the selected model. A no-op while
/// a pass is already running, and while the feature is switched off.
///
/// Which model runs is [`crate::settings::acoustic_source`], resolved here
/// rather than passed in: it's the same pick the similarity queries read, and
/// a caller that could hand in a different one would be able to fill the
/// table under a name nothing reads.
///
/// The database path comes in rather than the library entity, because the
/// watch sync starts this from inside the library's own update and reading a
/// leased entity panics.
pub fn start(db_path: PathBuf, cx: &mut App) {
    let settings = Settings::load();
    if progress(cx).is_some() || !settings.acoustic_analysis {
        return;
    }
    // Read once here rather than inside the pass: a pass keeps the worker
    // count it started with, and the next one picks up a changed setting.
    let workers = settings.acoustic_workers.max(1);
    let source = crate::settings::acoustic_source();
    let progress = Arc::new(Progress::new(source.id()));
    cx.set_global(Running(Some(progress.clone())));
    cx.set_global(LastFailure(None));
    // Keeps the menubar chip and the tasks window ticking; nothing observes
    // an app-global pass on its own.
    crate::tasks_window::repaint_while_running(cx);
    // Quitting mid-pass raises the same flag the stop button does, so the
    // workers land on a batch boundary instead of being killed mid-write.
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
        let result = cx
            .background_executor()
            .spawn({
                let progress = progress.clone();
                async move { rox_acoustic::run(&source, &db_path, workers, &progress) }
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
                Ok(written) => {
                    // The surfaces that offer ordering by sound are gated on
                    // there being vectors, and this is the moment there are.
                    if written > 0 {
                        crate::settings::set_acoustic_described(true, cx);
                    }
                    log::info!("acoustic: {written} tracks analyzed with {name}");
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
