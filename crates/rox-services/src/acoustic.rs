//! Which acoustic model is live: the resolution from a stored id to
//! something that can run, held in a process-global so every reader agrees.
//! The extractors and the catalog are in [`rox_acoustic`].

use std::sync::{Arc, RwLock};

use gpui::App;

use rox_acoustic::{Local, Source, models};
use rox_core::settings::{LocalModel, Settings, file_stamp};

/// A query against a different model from the one the pass filled would rank
/// against an empty corpus, so every reader goes through this.
static ACOUSTIC_MODEL: RwLock<Option<Source>> = RwLock::new(None);

/// None for a name from a newer build, a deleted download, or a moved local
/// file.
///
/// A local file is stamped, not just looked for: a checkpoint retrained in
/// place is a different vector space under the same path, and its vectors
/// must never be written under the old id.
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

/// A stale stamp costs one re-hash. A file that hashes to something else is a
/// different checkpoint and drops the pick to the built-in extractor.
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
        if let Some(stored) = s.acoustic_local_model.as_mut()
            && stored.path == path
        {
            stored.bytes = stamp.0;
            stored.mtime = stamp.1;
        }
    });
    true
}

/// Falls back to the built-in extractor here, not as an empty ranking
/// downstream.
pub fn acoustic_source() -> Source {
    ACOUSTIC_MODEL
        .read()
        .unwrap()
        .clone()
        .unwrap_or_else(|| Source::Catalog(models::fallback()))
}

/// The model the ML Models page offers, never the built-in one. Read from
/// the file, not the static: this is the pick the switch would turn on.
pub fn acoustic_ml_source() -> Source {
    let id = Settings::load().acoustic_ml_model;
    resolve_acoustic(&id)
        .filter(|source| !source.is_builtin())
        .or_else(|| {
            models::find(&id)
                .filter(|model| model.weights.is_some())
                .map(Source::Catalog)
        })
        .or_else(|| models::find(models::PANNS_CNN10).map(Source::Catalog))
        .unwrap_or_else(|| Source::Catalog(models::fallback()))
}

/// An unknown id isn't rewritten in the settings file: that would discard a
/// pick made by a newer build. Persisting is the caller's.
pub fn set_acoustic_model(id: &str, cx: &mut App) {
    *ACOUSTIC_MODEL.write().unwrap() = resolve_acoustic(id);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}
