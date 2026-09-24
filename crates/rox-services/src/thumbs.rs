//! The artwork service's front half per the components contract: a bounded
//! LRU of decoded thumbnails over [`rox_library::thumbs`]. A remote row asks
//! by the string that stands in for its path, which the store keys its
//! picture on. No request queue: visible rows re-ask every paint, and work
//! for rows that scrolled away is never picked back up. Evicted covers leave
//! gpui's asset cache explicitly; it never evicts on its own.
//!
//! A catalog change marks the cache stale instead of clearing it, so the
//! wall keeps painting while entries re-read and never flashes blank.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{App, Context, Entity, Image, ImageFormat, Subscription, Task};

use crate::catalog::{Library, LibraryEvent};

/// Sized for a full-screen album grid at the small tile size on 4K, with
/// headroom; below that the LRU thrashes every paint.
const CAP: usize = 512;
/// Warm loads are cheap, so this really caps concurrent cold decodes.
const POOL: usize = 16;
/// Kept low so the sweep never crowds the interactive pool.
const SWEEP_WORKERS: usize = 4;
/// A download saving tracks one at a time refreshes the catalog every few
/// seconds; without the settle the sweep restarts and never finishes.
const SWEEP_SETTLE: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub enum Thumb {
    Ready(Arc<Image>),
    Pending,
    /// A definitive answer, cached, so nothing re-asks.
    Missing,
}

struct Entry {
    image: Option<Arc<Image>>,
    touch: u64,
    /// A stale entry still paints; the next ask re-reads it and swaps only if
    /// the cover changed.
    fresh: bool,
}

pub struct Thumbs {
    /// None when the DB failed to open, which degrades every request to Missing.
    conn: Option<Arc<Mutex<rox_library::rusqlite::Connection>>>,
    entries: HashMap<PathBuf, Entry>,
    pending: HashSet<PathBuf>,
    clock: u64,
    generation: u64,
    sweep_cancel: Arc<AtomicBool>,
    sweep_settle: Option<Task<()>>,
    /// Never reset: a caller samples twice and takes the difference.
    stats: Stats,
    _library_changed: Subscription,
}

/// Counters for the `debug.thumbs` IPC method, to see whether loads still
/// land during a stall.
#[derive(Clone, Copy, Default, serde::Serialize)]
pub struct Stats {
    pub requests: u64,
    pub starts: u64,
    pub lands: u64,
    /// Declined because the pool was full or that path was already loading.
    pub refused: u64,
    pub pending: usize,
    pub entries: usize,
}

impl Thumbs {
    pub fn new(library: &Entity<Library>, cx: &mut Context<Self>) -> Self {
        // A rescan can rewrite art and id -> path mappings, so every texture
        // revalidates through the store's identity check while it keeps
        // painting.
        let _library_changed = cx.subscribe(
            library,
            |this: &mut Self, library, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.stale();
                this.queue_sweep(library, cx);
            },
        );
        let conn = rox_library::thumbs::open(&rox_core::settings::data_dir().join("thumbs.db"))
            .ok()
            .map(|conn| Arc::new(Mutex::new(conn)));
        Thumbs {
            conn,
            entries: HashMap::new(),
            pending: HashSet::new(),
            clock: 0,
            generation: 0,
            sweep_cancel: Arc::new(AtomicBool::new(false)),
            sweep_settle: None,
            stats: Stats::default(),
            _library_changed,
        }
    }

    /// Revalidate one path, for art that lands after the row cached Missing
    /// (a directory station painted before its favicon arrived). No
    /// generation bump, so reads in flight for other paths survive.
    pub fn forget(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get_mut(path) else {
            return;
        };

        entry.fresh = false;
        cx.notify();
    }

    pub fn stats(&self) -> Stats {
        Stats {
            pending: self.pending.len(),
            entries: self.entries.len(),
            ..self.stats
        }
    }

    /// Reports Pending while a load runs. A stale entry is served as-is while
    /// it re-reads.
    pub fn get(&mut self, path: &Path, cx: &mut Context<Self>) -> Thumb {
        self.clock += 1;
        self.stats.requests += 1;
        if let Some(entry) = self.entries.get_mut(path) {
            entry.touch = self.clock;
            let answer = match &entry.image {
                Some(image) => Thumb::Ready(image.clone()),
                None => Thumb::Missing,
            };
            if !entry.fresh {
                self.load(path, cx);
            }
            return answer;
        }
        if self.conn.is_none() {
            return Thumb::Missing;
        }
        self.load(path, cx);
        Thumb::Pending
    }

    fn load(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(conn) = &self.conn else {
            return;
        };
        if self.pending.contains(path) || self.pending.len() >= POOL {
            self.stats.refused += 1;
            return;
        }
        self.stats.starts += 1;
        self.pending.insert(path.to_path_buf());
        let generation = self.generation;
        let conn = conn.clone();
        let path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let bytes = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    // A server row with nothing stored fetches its cover
                    // here; the sweep never does, or it would download art
                    // for the whole wall. Pass the path, not its lossy
                    // string form, so a non-UTF-8 name stays itself.
                    async move {
                        rox_library::thumbs::thumbnail(&conn, &path)
                            .or_else(|| crate::sources::cover(&conn, &path.to_string_lossy()))
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending.remove(&path);
                this.land(path, bytes, cx);
                this.evict(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// gpui keys an image by its bytes' hash, so an unchanged cover shares the
    /// old handle's decode. Only release the old handle when the cover
    /// actually changed, or a bitmap still on screen drops.
    fn land(&mut self, path: PathBuf, bytes: Option<Vec<u8>>, cx: &mut App) {
        self.stats.lands += 1;
        let image = bytes.map(|b| Arc::new(Image::from_bytes(ImageFormat::Jpeg, b)));
        let retired = self
            .entries
            .get(&path)
            .and_then(|entry| entry.image.clone())
            .filter(|old| image.as_ref().is_none_or(|new| new.id() != old.id()));
        let touch = self.clock;
        self.entries.insert(
            path,
            Entry {
                image,
                touch,
                fresh: true,
            },
        );
        if let Some(old) = retired {
            old.remove_asset(cx);
        }
    }

    fn queue_sweep(&mut self, library: Entity<Library>, cx: &mut Context<Self>) {
        self.sweep_settle = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SWEEP_SETTLE).await;
            this.update(cx, |this, cx| this.sweep(&library, cx)).ok();
        }));
    }

    /// Warm the durable store for every album's first track, the grid's tile
    /// identity. Replaces any sweep still going.
    fn sweep(&mut self, library: &Entity<Library>, cx: &mut Context<Self>) {
        self.sweep_cancel.store(true, Ordering::Relaxed);
        let Some(conn) = self.conn.clone() else {
            return;
        };
        let ids = {
            let library = library.read(cx);
            // A refresh started during the settle wait queues its own sweep.
            if library.busy().is_some() {
                return;
            }
            let Some(projection) = library.projection() else {
                return;
            };
            let mut ids = Vec::new();
            let mut last = None;
            for &row in library.order().iter() {
                let key = (
                    projection.album_artist[row as usize],
                    projection.album[row as usize],
                );
                if last != Some(key) {
                    ids.push(projection.db_id[row as usize]);
                    last = Some(key);
                }
            }
            ids
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.sweep_cancel = cancel.clone();
        let db_path = rox_core::settings::data_dir().join("library.db");
        for chunk in ids.chunks(ids.len().div_ceil(SWEEP_WORKERS).max(1)) {
            let chunk = chunk.to_vec();
            let cancel = cancel.clone();
            let conn = conn.clone();
            let db_path = db_path.clone();
            cx.background_executor()
                .spawn(async move {
                    // Its own connection: the UI-side one stays on the UI thread.
                    let Ok(lib) = rox_library::store::open(&db_path) else {
                        return;
                    };
                    for id in chunk {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        let Ok(paths) = rox_library::store::paths_for(&lib, &[id]) else {
                            continue;
                        };
                        let Some(path) = paths.first() else {
                            continue;
                        };
                        rox_library::thumbs::thumbnail(&conn, Path::new(path));
                    }
                })
                .detach();
        }
    }

    /// For the settings window's clear, off the UI thread. The textures stay:
    /// they're still the right covers.
    pub fn store_conn(&self) -> Option<Arc<Mutex<rox_library::rusqlite::Connection>>> {
        self.conn.clone()
    }

    fn evict(&mut self, cx: &mut App) {
        while self.entries.len() > CAP {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.touch)
                .map(|(path, _)| path.clone())
            else {
                break;
            };
            if let Some(Entry {
                image: Some(image), ..
            }) = self.entries.remove(&oldest)
            {
                image.remove_asset(cx);
            }
        }
    }

    /// Every entry revalidates, none are dropped. Loads in flight read the
    /// old store, so they're orphaned.
    fn stale(&mut self) {
        for entry in self.entries.values_mut() {
            entry.fresh = false;
        }
        self.generation += 1;
        self.pending.clear();
    }
}
