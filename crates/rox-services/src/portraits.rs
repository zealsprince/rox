//! The artist portrait service: a bounded cache of decoded face
//! thumbnails over the durable store in [`crate::artists`], the shape
//! [`crate::thumbs`] takes for covers. Renders ask by artist name and get
//! a texture or nothing; a miss kicks a lookup on the background
//! executor, bounded to a few in flight. There is no request queue - a
//! visible tile re-asks every paint and a landing face repaints the
//! panels, so freed slots refill with whatever is still on screen.
//!
//! One service per workspace rather than one cache per view, so the
//! artist wall and the stats window share both the decodes and the
//! network round trips: faces browsed on the wall are already in hand
//! when the stats page opens. The LRU is sized to viewports, not the
//! library, and evicted faces leave gpui's asset cache explicitly, since
//! it never evicts on its own.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{App, Context, Image, ImageFormat};

use rox_net::providers;

use crate::artists;

/// Decoded portraits kept at once. Faces come from the artist store at
/// thumbnail size, so this is a few viewports' worth with headroom;
/// below that the LRU thrashes every paint.
const CAP: usize = 256;

/// Lookups in flight at once. Low on purpose: a cold one is a deezer
/// round trip, and a wall scrolled fast would otherwise fire hundreds at
/// a service that is doing us a favor.
const POOL: usize = 4;

/// One cached portrait; `image` None is an artist with no picture on
/// file and none to be had.
struct Entry {
    image: Option<Arc<Image>>,
    /// When the entry was last asked for, on the request clock; the LRU
    /// evicts the smallest.
    touch: u64,
}

#[derive(Default)]
pub struct Portraits {
    /// The decoded faces, keyed by the folded name the store answers on,
    /// so casing drift in the tags shares one entry.
    entries: HashMap<String, Entry>,
    /// Names with a lookup in flight; also the pool gauge.
    pending: HashSet<String>,
    /// The request clock behind [`Entry::touch`].
    clock: u64,
}

impl Portraits {
    /// An artist's portrait, from the cache or on its way. A miss starts
    /// a lookup when a pool slot is free and reports None either way; the
    /// landing notifies, so visible tiles re-ask and drain the misses
    /// without a queue. None also covers a settled miss, where the caller
    /// falls back to an album cover.
    pub fn get(&mut self, name: &str, cx: &mut Context<Self>) -> Option<Arc<Image>> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        // The store's own key: the folded name, except that
        // punctuation-only acts ("!!!", "+/-") fold to nothing and would
        // all share one entry, so those fall back to the raw name.
        let key = match providers::normalize(name) {
            folded if folded.is_empty() => name.to_string(),
            folded => folded,
        };
        self.clock += 1;
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.touch = self.clock;
            return entry.image.clone();
        }
        if self.pending.contains(&key) || self.pending.len() >= POOL {
            return None;
        }
        self.pending.insert(key.clone());
        let name = name.to_string();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let name = name.clone();
                    async move { artists::portrait_thumb(&name) }
                })
                .await;
            this.update(cx, |this, cx| {
                this.pending.remove(&key);
                // A network failure stays uncached, so the next look asks
                // again once the connection is back; only a settled answer
                // takes a slot.
                let image = match result {
                    Ok(bytes) => bytes.map(|b| Arc::new(Image::from_bytes(ImageFormat::Jpeg, b))),
                    Err(e) => {
                        log::debug!("portraits: {name}: {e}");
                        cx.notify();
                        return;
                    }
                };
                let touch = this.clock;
                this.entries.insert(key, Entry { image, touch });
                this.evict(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
        None
    }

    /// Trim the cache to [`CAP`], least-recently-asked first, releasing
    /// each evicted face from gpui's asset cache.
    fn evict(&mut self, cx: &mut App) {
        while self.entries.len() > CAP {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.touch)
                .map(|(key, _)| key.clone())
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
}
