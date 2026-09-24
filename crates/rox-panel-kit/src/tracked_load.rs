//! A path-keyed background image load with a generation guard, for panels
//! that fill a background off the file without blocking the UI thread.
//!
//! `img` keeps every distinct decode in gpui's process-wide asset cache and
//! never evicts, so the previous decode is retired on swap and on drop.
//! Without that a long session pins one full-size bitmap per album viewed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, Context, Image};

/// One decoded image for a track path, with a generation guard against a
/// track that turns over mid-read. The inner `None` is a track with no art.
#[derive(Default)]
pub struct TrackedImage {
    /// Kept so per-frame notifies never re-read the file.
    art: Option<(PathBuf, Option<Arc<Image>>)>,
    pending: Option<PathBuf>,
    generation: u64,
    /// Set by [`TrackedImage::refresh`]: what's held still paints until the
    /// next `ensure` re-reads it.
    stale: bool,
}

impl TrackedImage {
    pub fn get(&self, path: &Path) -> Option<Arc<Image>> {
        self.art
            .as_ref()
            .filter(|(cached, _)| cached.as_path() == path)
            .and_then(|(_, art)| art.clone())
    }

    /// Load `path`'s art off the UI thread unless it's cached or in flight.
    /// `slot` finds this tracker again inside the panel, since a field can't
    /// swap itself in.
    pub fn ensure<T, S, F>(&mut self, path: &Path, slot: S, decode: F, cx: &mut Context<T>)
    where
        T: 'static,
        S: Fn(&mut T) -> &mut TrackedImage + 'static,
        F: FnOnce() -> Option<Arc<Image>> + Send + 'static,
    {
        if self.pending.as_deref() == Some(path)
            || (!self.stale && self.art.as_ref().map(|(p, _)| p.as_path()) == Some(path))
        {
            return;
        }
        self.pending = Some(path.to_path_buf());
        self.generation += 1;
        let generation = self.generation;
        let path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let loaded = cx
                .background_executor()
                .spawn(async move { decode() })
                .await;
            this.update(cx, |this, cx| {
                let tracked = slot(this);
                if tracked.generation != generation {
                    return;
                }
                tracked.pending = None;
                tracked.stale = false;
                let old = tracked.art.take().and_then(|(_, art)| art);
                tracked.art = Some((path, loaded));
                tracked.retire(old, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A rescan can rewrite art under the same path: keep painting what's held
    /// and re-read on the next `ensure`. `retire` keys on the content id, so a
    /// re-read of the same cover never blanks the panel.
    pub fn refresh(&mut self) {
        self.stale = true;
        self.pending = None;
        self.generation += 1;
    }

    /// The panel's drop path. Takes the slot first so the retire always drops.
    pub fn invalidate(&mut self, cx: &mut App) {
        let old = self.art.take().and_then(|(_, art)| art);
        self.retire(old, cx);
    }

    /// Unless the slot now holds this same bitmap, which a re-read of the same
    /// bytes reuses.
    fn retire(&self, old: Option<Arc<Image>>, cx: &mut App) {
        let Some(old) = old else { return };
        if let Some((_, Some(current))) = &self.art
            && current.id() == old.id()
        {
            return;
        }
        old.remove_asset(cx);
    }
}
