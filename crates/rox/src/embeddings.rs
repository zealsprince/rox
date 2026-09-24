//! The acoustic analysis pass, as the app sees it: the app-global progress,
//! the last failure, and the spawn. The extractors, the model catalog, and
//! the pass itself are [`rox_acoustic`]. Same shape as
//! [`crate::replaygain_job`].

pub mod models;

use std::sync::Arc;

use gpui::{App, Entity, Global};

use rox_acoustic::Progress;
use rox_core::settings::Settings;
use rox_services::catalog::{Library, LibraryJob};

/// App-global so it outlives the settings window that started it.
#[derive(Default)]
struct Running(Option<Arc<Progress>>);

impl Global for Running {}

/// Kept after the pass is gone so the settings page can say why nothing
/// happened, as when a model's weights won't load.
#[derive(Default)]
struct LastFailure(Option<String>);

impl Global for LastFailure {}

pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

pub fn last_failure(cx: &App) -> Option<String> {
    cx.try_global::<LastFailure>().and_then(|f| f.0.clone())
}

pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel();
    }
}

/// Analyze every track with no vector for the selected model.
///
/// The model is resolved here, never passed in: the similarity queries read
/// the same pick, and any other would fill rows nothing reads.
///
/// Takes the library entity so tags-mode writes can be claimed through
/// [`Library::reindex_written`] before the watcher reindexes them. Safe to
/// call inside the library's own update: the entity isn't read until the
/// spawned task, since reading a leased entity panics.
pub fn start(library: Entity<Library>, cx: &mut App) {
    let settings = Settings::load();
    if progress(cx).is_some() || !settings.acoustic_analysis {
        return;
    }
    // Read once so a mid-pass settings flip can't split one album's vectors
    // between two destinations.
    let workers = settings.acoustic_workers.max(1);
    let save = settings.acoustic_save;
    let source = rox_services::acoustic::acoustic_source();
    let progress = Arc::new(Progress::new(source.id()));
    cx.set_global(Running(Some(progress.clone())));
    cx.set_global(LastFailure(None));
    // Nothing observes an app-global pass on its own.
    crate::tasks_window::repaint_while_running(cx);
    // Quit stops the workers on a batch boundary, not mid-write.
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
        // Read here, not up top, so a caller inside the library's update is safe.
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
            // Worker-seconds per track, for the Library page's estimates. Only off a
            // decent stretch: a short pass measures its own startup.
            if progress.done() >= rox_acoustic::PACE_FLOOR
                && let Some(per) = progress.secs_per_track()
            {
                let pace = (per * workers as f64) as f32;
                let id = name.clone();
                Settings::update(move |s| {
                    s.session.acoustic_pace.insert(id, pace);
                });
            }
            match result {
                Ok(analyzed) => {
                    if analyzed.described > 0 {
                        rox_core::settings::set_acoustic_described(true, cx);
                    }
                    log::info!(
                        "acoustic: {} tracks analyzed with {name}, {} tagged",
                        analyzed.described,
                        analyzed.tagged.len()
                    );
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

/// Analyze what each watch sync brings in, while the auto switch is on. The
/// switch is read here rather than in [`start`] so the button still works
/// with it off. Full scans don't trigger this; the catalog only emits for
/// the watch case.
pub fn follow(library: &Entity<Library>, cx: &mut App) {
    App::subscribe(cx, library, |library, event, cx| {
        if matches!(event, LibraryJob::WatchSettled) && Settings::load().acoustic_auto {
            start(library, cx);
        }
    })
    .detach();
}
