//! The artist portrait service: a bounded LRU of decoded face thumbnails
//! over [`crate::artists`], the same shape [`crate::thumbs`] has for covers.
//! No request queue: visible tiles re-ask every paint. One per workspace, so
//! the artist wall and the stats window share decodes and network trips.
//! Evicted faces leave gpui's asset cache explicitly; it never evicts itself.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{App, Context, Image, ImageFormat};

use rox_net::providers;

use crate::artists;

/// A few viewports of faces; below that the LRU thrashes every paint.
const CAP: usize = 256;

/// Kept low: a fast-scrolled wall would otherwise fire hundreds of deezer
/// lookups at once.
const POOL: usize = 4;

struct Entry {
    image: Option<Arc<Image>>,
    touch: u64,
}

#[derive(Default)]
pub struct Portraits {
    entries: HashMap<String, Entry>,
    pending: HashSet<String>,
    clock: u64,
}

impl Portraits {
    /// None while a lookup runs and for a settled miss; the finished lookup
    /// notifies, so visible tiles re-ask.
    pub fn get(&mut self, name: &str, cx: &mut Context<Self>) -> Option<Arc<Image>> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        // Punctuation-only names ("!!!") fold to nothing, so key those raw.
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
                // A network failure stays uncached so the next look retries.
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
