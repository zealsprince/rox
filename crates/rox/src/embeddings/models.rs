//! The model manager as the app sees it: the catalog, the install checks,
//! and the download all live in [`rox_acoustic::models`] and are re-exported
//! below. What stays here is the app-global half of a running download, the
//! same shape [`super`] keeps for the pass.

use std::sync::Arc;

use gpui::{App, Global};

use rox_acoustic::models::{fetch, Model, Progress};

/// The running download, or nothing. App-global so it outlives the settings
/// window that started it.
#[derive(Default)]
struct Running(Option<Arc<Progress>>);

impl Global for Running {}

/// The last download's failure, kept after the download itself is gone so
/// the settings page can still say what went wrong.
#[derive(Default)]
struct LastFailure(Option<(String, String)>);

impl Global for LastFailure {}

/// The running download's progress, for a UI that wants to show it.
pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

/// What the last download failed with, as (model id, reason). Cleared when
/// a new download starts.
pub fn last_failure(cx: &App) -> Option<(String, String)> {
    cx.try_global::<LastFailure>().and_then(|f| f.0.clone())
}

/// Ask the running download to stop. The part file goes with it, so a stop
/// leaves nothing half-written behind.
pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel();
    }
}

/// Fetch a model's weights. A no-op while a download is already running, and
/// for a model that has nothing to fetch.
pub fn start(model: &'static Model, cx: &mut App) {
    if progress(cx).is_some() || model.weights.is_none() {
        return;
    }
    let progress = Arc::new(Progress::new(model));
    cx.set_global(Running(Some(progress.clone())));
    cx.set_global(LastFailure(None));
    // Quitting mid-download shouldn't leave a part file behind, and the
    // worker deletes one on the way out of a cancelled fetch.
    cx.on_app_quit({
        let progress = progress.clone();
        move |_| {
            progress.cancel();
            async {}
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        let outcome = cx
            .background_executor()
            .spawn({
                let progress = progress.clone();
                async move { fetch(model, &progress) }
            })
            .await;
        cx.update(|cx| {
            if let Err(reason) = outcome {
                log::error!("model download: {}: {reason}", model.id);
                cx.set_global(LastFailure(Some((model.id.to_string(), reason))));
            } else {
                log::info!("model download: {} installed", model.id);
            }
            cx.set_global(Running(None));
        })
        .ok();
    })
    .detach();
}
