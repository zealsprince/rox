//! The Japanese dictionary as the app sees it: the descriptor, the install
//! check and the download are all defined in [`rox_romanize::dictionary`]
//! and re-exported below. What stays here is the app-global half of a
//! running download, the same shape [`crate::embeddings::models`] keeps for
//! the acoustic weights.

use std::sync::Arc;

use gpui::{App, Global};

use rox_romanize::dictionary::{fetch, Dictionary, Progress};

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

/// The running download's progress, for a UI that shows it.
pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

/// What the last download failed with, as (dictionary id, reason). Cleared
/// when a new download starts.
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

/// Fetch and unpack a dictionary. A no-op while a download is already
/// running.
pub fn start(dictionary: &'static Dictionary, cx: &mut App) {
    if progress(cx).is_some() {
        return;
    }
    let progress = Arc::new(Progress::new(dictionary));
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
                async move { fetch(dictionary, &progress) }
            })
            .await;
        cx.update(|cx| {
            if let Err(reason) = outcome {
                log::error!("dictionary download: {}: {reason}", dictionary.id);
                cx.set_global(LastFailure(Some((dictionary.id.to_string(), reason))));
            } else {
                log::info!("dictionary download: {} installed", dictionary.id);
                // The shared dictionary remembers that there was none to
                // load; without this the install doesn't take until a
                // restart.
                rox_romanize::reload();
            }
            cx.set_global(Running(None));
        })
        .ok();
    })
    .detach();
}
