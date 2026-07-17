//! The artwork service's front half per the components contract: a
//! bounded cache of decoded thumbnail textures over the durable store in
//! [`rox_library::thumbs`]. Renders ask by track path and get a texture,
//! a pending marker, or a definitive miss; a miss kicks a load on the
//! background executor, bounded to a few in flight. There is no request
//! queue: a visible row re-asks every paint and a landing load repaints
//! the panels, so freed slots refill with whatever is still on screen -
//! work for rows that scrolled away is simply never picked back up,
//! which is the contract's off-screen cancellation. The texture cache is
//! an LRU sized to viewports, not the library, and evicted covers leave
//! gpui's asset cache explicitly, since it never evicts on its own (the
//! cover panel's lesson).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{App, Context, Entity, Image, ImageFormat, Subscription};

use crate::panels::library::{Library, LibraryEvent};

/// Decoded textures kept at once: a few viewports of tiles, not the
/// library. Sized for the hungriest consumer, a full-screen album grid
/// at the small tile size on a 4K display, with headroom; below that
/// the LRU thrashes every paint.
const CAP: usize = 512;
/// Loads in flight at once, the contract's bounded worker pool. Warm
/// loads are stat-plus-point-lookup cheap, so the bound is really a
/// cap on concurrent cover decodes when the store is cold.
const POOL: usize = 16;
/// Background tasks the post-refresh sweep splits the wall across. Low
/// on purpose: the sweep warms the durable store for tiles nobody is
/// looking at yet, so it should never crowd the interactive pool or
/// the machine.
const SWEEP_WORKERS: usize = 4;

/// What a render gets for a track's thumbnail.
pub enum Thumb {
    Ready(Arc<Image>),
    /// A load is in flight or waiting on a slot; re-ask next paint.
    Pending,
    /// The track has no art (or the store is unavailable): a definitive
    /// answer, cached, so nothing re-asks.
    Missing,
}

/// One cached answer; `image` None is a known no-art track.
struct Entry {
    image: Option<Arc<Image>>,
    /// When the entry was last asked for, on the request clock; the LRU
    /// evicts the smallest.
    touch: u64,
}

/// The shared thumbnail service, one per workspace through
/// [`AppState`](crate::panel::AppState).
pub struct Thumbs {
    /// The store connection shared across workers; None when the DB
    /// failed to open, which degrades every request to Missing.
    conn: Option<Arc<Mutex<rox_library::rusqlite::Connection>>>,
    entries: HashMap<PathBuf, Entry>,
    /// Paths with a load in flight; also the pool gauge.
    pending: HashSet<PathBuf>,
    /// The request clock behind `Entry::touch`.
    clock: u64,
    /// Discards in-flight results from before an invalidation.
    generation: u64,
    /// The running store sweep's stop flag; a new sweep raises it and
    /// leaves a fresh one behind.
    sweep_cancel: Arc<AtomicBool>,
    _library_changed: Subscription,
}

impl Thumbs {
    pub fn new(library: &Entity<Library>, cx: &mut Context<Self>) -> Self {
        // A rescan can rewrite tags, art files, and id -> path mappings;
        // drop the textures so the next paints re-read through the
        // store's (path, mtime, size) identity check. A settled catalog
        // also kicks the sweep that warms the store for the whole wall.
        let _library_changed =
            cx.subscribe(library, |this: &mut Self, library, event: &LibraryEvent, cx| {
                if !matches!(event, LibraryEvent::Updated) {
                    return;
                }
                this.invalidate(cx);
                this.sweep(&library, cx);
            });
        let conn = rox_library::thumbs::open(&crate::settings::data_dir().join("thumbs.db"))
            .ok()
            .map(|conn| Arc::new(Mutex::new(conn)));
        Thumbs {
            conn,
            entries: HashMap::new(),
            pending: HashSet::new(),
            clock: 0,
            generation: 0,
            sweep_cancel: Arc::new(AtomicBool::new(false)),
            _library_changed,
        }
    }

    /// The thumbnail for `path`, from cache or on its way. A miss starts
    /// a load when a pool slot is free and reports Pending either way;
    /// the landing notifies, so visible rows re-ask and drain the misses
    /// without a queue.
    pub fn get(&mut self, path: &Path, cx: &mut Context<Self>) -> Thumb {
        self.clock += 1;
        if let Some(entry) = self.entries.get_mut(path) {
            entry.touch = self.clock;
            return match &entry.image {
                Some(image) => Thumb::Ready(image.clone()),
                None => Thumb::Missing,
            };
        }
        let Some(conn) = &self.conn else {
            return Thumb::Missing;
        };
        if self.pending.contains(path) || self.pending.len() >= POOL {
            return Thumb::Pending;
        }
        self.pending.insert(path.to_path_buf());
        let generation = self.generation;
        let conn = conn.clone();
        let path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let bytes = cx
                .background_executor()
                .spawn({
                    let path = path.clone();
                    async move { rox_library::thumbs::thumbnail(&conn, &path) }
                })
                .await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending.remove(&path);
                let image = bytes.map(|b| Arc::new(Image::from_bytes(ImageFormat::Jpeg, b)));
                let touch = this.clock;
                this.entries.insert(path, Entry { image, touch });
                this.evict(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        Thumb::Pending
    }

    /// Warm the durable store for the whole wall: every album's first
    /// track, the tile identity the grid loads by, gets its thumbnail
    /// generated in the background. Unchanged covers are point lookups,
    /// so a warm sweep is light; a cold one pays each decode once here
    /// instead of on first scroll-by. Runs after every library refresh
    /// and replaces any sweep still going.
    fn sweep(&mut self, library: &Entity<Library>, cx: &mut Context<Self>) {
        self.sweep_cancel.store(true, Ordering::Relaxed);
        let Some(conn) = self.conn.clone() else {
            return;
        };
        let ids = {
            let library = library.read(cx);
            // Mid-refresh the projection is still the old catalog; the
            // completion event kicks the sweep that matters.
            if library.busy().is_some() {
                return;
            }
            let Some(projection) = library.projection() else {
                return;
            };
            // First row of each album run, the grid's grouping rule.
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
        let db_path = crate::settings::data_dir().join("library.db");
        for chunk in ids.chunks(ids.len().div_ceil(SWEEP_WORKERS).max(1)) {
            let chunk = chunk.to_vec();
            let cancel = cancel.clone();
            let conn = conn.clone();
            let db_path = db_path.clone();
            cx.background_executor()
                .spawn(async move {
                    // Its own library connection, the scan idiom: the
                    // UI-side one stays on the UI thread.
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

    /// The shared store connection, for the settings window to clear the
    /// durable cache off the UI thread; the Mutex serializes that against
    /// in-flight loads. The textures stay put, they are still the right
    /// covers. None when the DB failed to open.
    pub fn store_conn(&self) -> Option<Arc<Mutex<rox_library::rusqlite::Connection>>> {
        self.conn.clone()
    }

    /// Trim the cache to [`CAP`], least-recently-asked first, releasing
    /// each evicted cover's decoded bitmap from gpui's asset cache.
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

    /// Drop everything cached and orphan in-flight loads; the durable
    /// store stays, so re-reads for unchanged files are DB hits.
    fn invalidate(&mut self, cx: &mut Context<Self>) {
        for (_, entry) in self.entries.drain() {
            if let Some(image) = entry.image {
                image.remove_asset(cx);
            }
        }
        self.generation += 1;
        self.pending.clear();
        cx.notify();
    }
}
