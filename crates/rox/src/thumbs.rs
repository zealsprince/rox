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
use std::sync::{Arc, Mutex};

use gpui::{App, Context, Entity, Image, ImageFormat, Subscription};

use crate::panels::library::{Library, LibraryEvent};

/// Decoded textures kept at once: a few viewports of header tiles, not
/// the library.
const CAP: usize = 128;
/// Loads in flight at once, the contract's bounded worker pool.
const POOL: usize = 4;

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
    _library_changed: Subscription,
}

impl Thumbs {
    pub fn new(library: &Entity<Library>, cx: &mut Context<Self>) -> Self {
        // A rescan can rewrite tags, art files, and id -> path mappings;
        // drop the textures so the next paints re-read through the
        // store's (path, mtime, size) identity check.
        let _library_changed = cx.subscribe(library, |this: &mut Self, _, _: &LibraryEvent, cx| {
            this.invalidate(cx)
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
