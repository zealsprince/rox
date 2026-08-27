//! Which acoustic model is live. The extractors, the catalog, and the
//! download are all in [`rox_acoustic`]; what's here is the resolution
//! between a stored id and something that can actually run, held in a
//! process-global so the catalog, the player's similarity draws, and the
//! settings page all read the same value.

use std::sync::{Arc, RwLock};

use gpui::App;

use rox_acoustic::{models, Local, Source};
use rox_core::settings::{file_stamp, LocalModel, Settings};

/// The live model pick, the acoustic switch's other half: the Similar column
/// reads it in the same render paths, and a query that used a different
/// model from the one the pass filled would rank against an empty corpus.
static ACOUSTIC_MODEL: RwLock<Option<Source>> = RwLock::new(None);

/// Resolve a stored model id to something that can actually run: a catalog
/// entry whose weights are installed, or the local file the user picked.
/// None for a name from a newer build, one whose download has since been
/// deleted, or a local file that has moved.
///
/// A local id names the bytes it was hashed from, so the file is stamped
/// rather than only looked for: a checkpoint retrained in place is a different
/// vector space under the same path, and writing its vectors under the old
/// id is the mixing that hashed naming exists to prevent. The stat keeps that
/// off the common path, so only a file that actually changed pays a re-read.
pub fn resolve_acoustic(id: &str) -> Option<Source> {
    if let Some(model) = models::find(id).filter(|model| model.installed()) {
        return Some(Source::Catalog(model));
    }
    let local = Settings::load()
        .acoustic_local_model
        .filter(|local| local.id == id)?;
    let stamp = file_stamp(&local.path)?;
    if stamp != (local.bytes, local.mtime) && !rehashes_to_its_id(&local, stamp) {
        return None;
    }
    Some(Source::Local(Arc::new(Local {
        path: local.path,
        id: local.id,
    })))
}

/// Whether a weights file that no longer matches its stamp still hashes to the
/// id it's stored under, recording what it looks like now when it does: a
/// stamp from before the pair was written, or an mtime a copy or a touch
/// moved, then costs one read rather than one on every resolve.
///
/// A file that hashes to something else is a different checkpoint and gets
/// nothing, which drops the pick to the built-in extractor rather than filling
/// the old id with the new network's coordinates. Pointing rox at the file
/// again adopts it under its own name, with the work already done under the
/// old one still sitting there.
fn rehashes_to_its_id(local: &LocalModel, stamp: (u64, i64)) -> bool {
    let Ok(digest) = models::hash_file(&local.path) else {
        return false;
    };
    if rox_acoustic::local_id(&digest) != local.id {
        log::warn!(
            "settings: {} is no longer the checkpoint {} was named after",
            local.path.display(),
            local.id
        );
        return false;
    }
    let path = local.path.clone();
    Settings::update(move |s| {
        if let Some(stored) = s.acoustic_local_model.as_mut() {
            if stored.path == path {
                stored.bytes = stamp.0;
                stored.mtime = stamp.1;
            }
        }
    });
    true
}

/// The model the pass runs and the similarity queries read.
///
/// Resolved once into the static rather than at every call site, so a name
/// from a newer build, or one whose weights have gone missing since, falls
/// back to the built-in extractor here instead of turning into an empty ranking
/// somewhere downstream.
pub fn acoustic_source() -> Source {
    ACOUSTIC_MODEL
        .read()
        .unwrap()
        .clone()
        .unwrap_or_else(|| Source::Catalog(models::fallback()))
}

/// The model the ML Models page is offering, which the Library page's
/// extractor switch turns on. Never the built-in extractor: that one is the
/// other side of the switch rather than something the shelf offers.
///
/// Read from the file rather than the static above, because this is the pick
/// the switch would turn on rather than the one running now, and the two are
/// different whenever the switch is on Built-in.
pub fn acoustic_ml_source() -> Source {
    let id = Settings::load().acoustic_ml_model;
    resolve_acoustic(&id)
        .filter(|source| !source.is_builtin())
        // An id that resolves to nothing still names a model the page can
        // show as not-yet-downloaded, so fall back to the catalog entry
        // before falling back to PANNs.
        .or_else(|| {
            models::find(&id)
                .filter(|model| model.weights.is_some())
                .map(Source::Catalog)
        })
        .or_else(|| models::find(models::PANNS_CNN10).map(Source::Catalog))
        .unwrap_or_else(|| Source::Catalog(models::fallback()))
}

/// Point the live pick at a model by id and repaint, so a switch moves the
/// Similar column onto the other model's vectors without a relaunch. An
/// unknown id resolves to nothing and the reader above falls back; it isn't
/// rewritten in the settings file, since that would silently discard a pick
/// made by a newer build. Persisting is the caller's.
pub fn set_acoustic_model(id: &str, cx: &mut App) {
    *ACOUSTIC_MODEL.write().unwrap() = resolve_acoustic(id);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}
