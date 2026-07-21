//! Filesystem watching for the library roots, the live half of the library
//! contract's `watch(on/off)`. notify runs its own OS-backed watcher thread
//! and the debouncer folds a burst - a bulk copy, a directory deletion - into
//! batches before it hands them here, so the library never thrashes one file
//! at a time. Each batch of changed paths rides an async channel to the
//! library entity, which maps them onto the same scan/upsert/prune path a
//! manual rescan takes. Dropping the handle stops the watch: the debouncer's
//! thread ends and the channel closes.

use std::path::PathBuf;
use std::time::Duration;

use notify_debouncer_full::notify::event::{ModifyKind, RenameMode};
use notify_debouncer_full::notify::{EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, RecommendedCache};

/// How long a path has to sit quiet before the debouncer flushes it. Long
/// enough that a bulk copy's writes fold together, short enough that a single
/// edit lands in the library within a couple of seconds.
const DEBOUNCE: Duration = Duration::from_millis(1000);

/// One flushed batch of changes: the plain changed paths and, apart from them,
/// the renames the debouncer managed to correlate. A correlated rename carries
/// the (from, to) pair so the library can move the row and keep its id, rather
/// than watch the old path die and the new one land fresh. Anything not a
/// correlated rename - a create, a modify, a delete, or a rename we could not
/// pair - rides `paths`.
pub struct WatchBatch {
    pub paths: Vec<PathBuf>,
    pub renames: Vec<(PathBuf, PathBuf)>,
}

/// The notify debouncer plus the receiver its callback feeds. Held by the
/// library for as long as watching is on; dropping it tears the watcher down.
pub struct LibraryWatcher {
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
    events: async_channel::Receiver<WatchBatch>,
    /// How many roots the watch actually came up over, and how many were
    /// asked for, so the library can report a partial watch.
    watched: usize,
    total: usize,
}

impl LibraryWatcher {
    /// Arm a recursive watch over every root. `None` when the platform
    /// watcher will not come up, so the app runs on without live updates
    /// rather than failing; a root that cannot be watched (missing folder,
    /// unplugged drive) is skipped, the others still watched.
    pub fn new(roots: &[PathBuf]) -> Option<LibraryWatcher> {
        let (tx, events) = async_channel::unbounded();
        let mut debouncer = new_debouncer(
            DEBOUNCE,
            None,
            move |result: DebounceEventResult| {
                // Runs on the debouncer's own thread. Access events (a plain
                // read, the file we just played) never change the catalog, so
                // they are dropped here; create, modify, and remove carry the
                // paths a rescan needs to converge. Errors just skip the batch;
                // the next real change re-triggers the sync.
                let Ok(batch) = result else {
                    return;
                };
                let mut paths: Vec<PathBuf> = Vec::new();
                let mut renames: Vec<(PathBuf, PathBuf)> = Vec::new();
                for event in batch {
                    if matches!(event.kind, EventKind::Access(_)) {
                        continue;
                    }
                    // The debouncer correlates a rename into a single Both
                    // event carrying [from, to]. Carry that as a pair so the
                    // sync moves the row and keeps its id; a Both that did not
                    // land exactly two paths is not a pair we can trust, so it
                    // falls back into the plain path list.
                    if matches!(event.kind, EventKind::Modify(ModifyKind::Name(RenameMode::Both))) {
                        if let [from, to] = event.paths.as_slice() {
                            renames.push((from.clone(), to.clone()));
                            continue;
                        }
                    }
                    paths.extend(event.paths.iter().cloned());
                }
                if !paths.is_empty() || !renames.is_empty() {
                    let _ = tx.try_send(WatchBatch { paths, renames });
                }
            },
        )
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

    /// A receiver clone for the library's drain loop.
    pub fn events(&self) -> async_channel::Receiver<WatchBatch> {
        self.events.clone()
    }

    /// How many roots the watch came up over versus how many were asked for.
    /// A partial count means some roots failed to watch (missing folder,
    /// unplugged drive) while the rest stay live.
    pub fn coverage(&self) -> (usize, usize) {
        (self.watched, self.total)
    }
}
