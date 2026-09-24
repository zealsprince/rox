//! Filesystem watching for the library roots. The debouncer folds bursts
//! into batches, which go over a channel to the library entity and take the
//! same upsert/prune path a rescan does. Dropping the handle stops the watch.

use std::path::PathBuf;
use std::time::Duration;

use notify_debouncer_full::notify::event::{ModifyKind, RenameMode};
use notify_debouncer_full::notify::{EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};

use crate::writer;

/// Long enough to fold a bulk copy, short enough that an edit lands within
/// a couple of seconds.
const DEBOUNCE: Duration = Duration::from_millis(1000);

/// One flushed batch: plain changed paths, and the renames the debouncer
/// correlated, kept as pairs so the row keeps its id.
pub struct WatchBatch {
    pub paths: Vec<PathBuf>,
    pub renames: Vec<(PathBuf, PathBuf)>,
}

pub struct LibraryWatcher {
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    events: async_channel::Receiver<WatchBatch>,
    watched: usize,
    total: usize,
}

impl LibraryWatcher {
    /// `None` when the platform watcher won't come up; a root that can't be
    /// watched is skipped.
    pub fn new(roots: &[PathBuf]) -> Option<LibraryWatcher> {
        let (tx, events) = async_channel::unbounded();
        let mut debouncer = new_debouncer(DEBOUNCE, None, move |result: DebounceEventResult| {
            // Access events never change the catalog. A failed batch is skipped; the
            // next change re-triggers the sync.
            let Ok(batch) = result else {
                return;
            };
            let mut paths: Vec<PathBuf> = Vec::new();
            let mut renames: Vec<(PathBuf, PathBuf)> = Vec::new();
            for event in batch {
                if matches!(event.kind, EventKind::Access(_)) {
                    continue;
                }
                // No extension filter: a .cue edit re-cuts its image, and
                // `scanner::is_relevant` passes it. A correlated rename arrives as one Both
                // event with [from, to].
                if matches!(
                    event.kind,
                    EventKind::Modify(ModifyKind::Name(RenameMode::Both))
                ) && let [from, to] = event.paths.as_slice()
                {
                    // The writer's clone renamed over the original is a modify, not a rename.
                    // Passed as a rename it would dodge the self-write filter.
                    if writer::is_clone_path(from) {
                        paths.push(to.clone());
                    } else {
                        renames.push((from.clone(), to.clone()));
                    }
                    continue;
                }
                // The writer's clones are never library rows.
                paths.extend(
                    event
                        .paths
                        .iter()
                        .filter(|p| !writer::is_clone_path(p))
                        .cloned(),
                );
            }
            if !paths.is_empty() || !renames.is_empty() {
                let _ = tx.try_send(WatchBatch { paths, renames });
            }
        })
        .ok()?;
        let mut watched = 0;
        for root in roots {
            if debouncer.watch(root, RecursiveMode::Recursive).is_ok() {
                watched += 1;
            }
        }
        Some(LibraryWatcher {
            _debouncer: debouncer,
            events,
            watched,
            total: roots.len(),
        })
    }

    pub fn events(&self) -> async_channel::Receiver<WatchBatch> {
        self.events.clone()
    }

    pub fn coverage(&self) -> (usize, usize) {
        (self.watched, self.total)
    }
}
