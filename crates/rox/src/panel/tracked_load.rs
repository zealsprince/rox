//! A path-keyed background image load with a generation guard, the block
//! several panels hand-rolled to fill a background off the file without
//! blocking the UI thread. It reads the file on the background executor,
//! discards a result whose track changed mid-read, and retires the previous
//! decode when a new one swaps in and again when the panel is dropped, so a
//! cover never lingers in gpui's process-wide, never-evicting asset cache.
//!
//! Covers reach the renderer through `img`, which keeps every distinct
//! decode in that cache and never evicts on its own, so without the retires
//! a long session pins one full-size bitmap per album viewed and a closed
//! panel leaks whatever it last showed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, Context, Image};

/// A single decoded image loaded for a track path, with the machinery to
/// keep it in step: a pending marker so a render can tell "already fetching"
/// from "needs a fetch", and a generation counter that discards a stale
/// result when the track turns over mid-read. The held decode is `None`
/// inside when the track carries no art.
#[derive(Default)]
pub struct TrackedImage {
    /// The loaded decode keyed by the track it belongs to; None inside means
    /// the track has no art. Kept so per-frame notifies never re-read the
    /// file.
    art: Option<(PathBuf, Option<Arc<Image>>)>,
    /// The track a load is running for.
    pending: Option<PathBuf>,
    /// Discards stale load results when the track changes mid-read.
    generation: u64,
}

impl TrackedImage {
    /// The decode held for `path`, or None while still loading or when the
    /// track carries none.
    pub fn get(&self, path: &Path) -> Option<Arc<Image>> {
        self.art
            .as_ref()
            .filter(|(cached, _)| cached.as_path() == path)
            .and_then(|(_, art)| art.clone())
    }

    /// Make sure the art for `path` is cached or on its way: run `decode`
    /// off the UI thread and swap the result in when it lands, discarding it
    /// if the track moved on. `decode` reads the file and returns the decode,
    /// or None when the track has no art. `slot` reaches this tracker back
    /// inside the panel when the load lands, since the tracker is a field of
    /// the panel it cannot swap itself in.
    pub fn ensure<T, S, F>(&mut self, path: &Path, slot: S, decode: F, cx: &mut Context<T>)
    where
        T: 'static,
        S: Fn(&mut T) -> &mut TrackedImage + 'static,
        F: FnOnce() -> Option<Arc<Image>> + Send + 'static,
    {
        if self.art.as_ref().map(|(p, _)| p.as_path()) == Some(path)
            || self.pending.as_deref() == Some(path)
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
                let old = tracked.art.take().and_then(|(_, art)| art);
                tracked.art = Some((path, loaded));
                tracked.retire(old, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Forget the held load and retire its decode. Serves both a library
    /// update, which can rewrite art on disc under the same path so the next
    /// render must re-read, and the panel's drop path, so a closed panel
    /// leaves nothing pinned in the asset cache. Takes the slot first so the
    /// retire sees no current decode and always drops.
    pub fn invalidate(&mut self, cx: &mut App) {
        let old = self.art.take().and_then(|(_, art)| art);
        self.retire(old, cx);
    }

    /// Drop a replaced decode from gpui's asset cache, unless the same
    /// bitmap is what the slot holds now, which a re-read of the same bytes
    /// reuses.
    fn retire(&self, old: Option<Arc<Image>>, cx: &mut App) {
        let Some(old) = old else { return };
        if let Some((_, Some(current))) = &self.art {
            if current.id() == old.id() {
                return;
            }
        }
        old.remove_asset(cx);
    }
}
