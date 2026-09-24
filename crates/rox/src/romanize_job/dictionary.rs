//! The app-global half of a Japanese dictionary download; everything else lives
//! in [`rox_romanize::dictionary`].

use std::sync::Arc;

use gpui::{App, Global};

use rox_romanize::dictionary::{Dictionary, Progress, fetch};

#[derive(Default)]
struct Running(Option<Arc<Progress>>);

impl Global for Running {}

#[derive(Default)]
struct LastFailure(Option<(String, String)>);

impl Global for LastFailure {}

pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

/// As (dictionary id, reason). Cleared when a new download starts.
pub fn last_failure(cx: &App) -> Option<(String, String)> {
    cx.try_global::<LastFailure>().and_then(|f| f.0.clone())
}

pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel();
    }
}

pub fn start(dictionary: &'static Dictionary, cx: &mut App) {
    if progress(cx).is_some() {
        return;
    }
    let progress = Arc::new(Progress::new(dictionary));
    cx.set_global(Running(Some(progress.clone())));
    cx.set_global(LastFailure(None));
    // Quitting cancels, and a cancelled fetch deletes its part file.
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
                async move { fetch(dictionary, &progress) }
            })
            .await;
        cx.update(|cx| {
            if let Err(reason) = outcome {
                log::error!("dictionary download: {}: {reason}", dictionary.id);
                cx.set_global(LastFailure(Some((dictionary.id.to_string(), reason))));
            } else {
                log::info!("dictionary download: {} installed", dictionary.id);
                // The shared dictionary caches that there was none, so reload
                // or the install waits for a restart.
                rox_romanize::reload();
            }
            cx.set_global(Running(None));
        })
        .ok();
    })
    .detach();
}
