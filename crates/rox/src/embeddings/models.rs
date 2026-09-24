//! The app-global half of a model download. The catalog, install checks,
//! and the download itself are [`rox_acoustic::models`].

use std::sync::Arc;

use gpui::{App, Global};

use rox_acoustic::models::{Model, Progress, fetch};

/// App-global so it outlives the settings window that started it.
#[derive(Default)]
struct Running(Option<Arc<Progress>>);

impl Global for Running {}

#[derive(Default)]
struct LastFailure(Option<(String, String)>);

impl Global for LastFailure {}

pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

/// (model id, reason). Cleared when a new download starts.
pub fn last_failure(cx: &App) -> Option<(String, String)> {
    cx.try_global::<LastFailure>().and_then(|f| f.0.clone())
}

/// The part file is deleted with the cancelled fetch.
pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel();
    }
}

pub fn start(model: &'static Model, cx: &mut App) {
    if progress(cx).is_some() || model.weights.is_none() {
        return;
    }
    let progress = Arc::new(Progress::new(model));
    cx.set_global(Running(Some(progress.clone())));
    cx.set_global(LastFailure(None));
    // Cancel on quit so the worker deletes the part file.
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
