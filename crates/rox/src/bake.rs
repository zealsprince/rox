//! Embedding stored metadata: the survey and the run behind
//! [`crate::bake_dialog`], around [`rox_library::bake`]. Same shape and pool
//! as [`crate::convert`]: every write is a whole-file clone-verify-rename, so
//! this is disk-bound and going wide only slows it.
//!
//! Nothing is computed here. Every value was already in the database, the
//! lyrics store, or a sidecar, so a second run finds the tags and skips.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{App, Entity, Global};

use rox_library::bake::{self, Candidate, Item};
use rox_library::store;
use rox_services::catalog::Library;

/// A tag commit is a whole-file copy; four already saturate a disk.
const MAX_WORKERS: usize = 4;

fn workers(len: usize) -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get() / 2)
        .unwrap_or(1)
        .clamp(1, MAX_WORKERS)
        .min(len.max(1))
}

#[derive(Default)]
pub struct Survey {
    done: AtomicUsize,
    total: AtomicUsize,
    cancel: AtomicBool,
}

impl Survey {
    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    /// Zero while the database half is still running.
    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    pub fn abandon(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn keep_going(&self) -> bool {
        !self.cancel.load(Ordering::Relaxed)
    }
}

/// Everything a bake could write, refusals included. Blocking: one file open
/// per candidate. A cancelled survey returns partial counts to be discarded.
pub fn survey(
    db_path: &Path,
    model: &str,
    lyrics_dir: Option<&Path>,
    progress: &Survey,
) -> Result<Vec<Candidate>, String> {
    let conn = store::open(db_path).map_err(|e| e.to_string())?;
    let mut found = bake::candidates(&conn, model, lyrics_dir).map_err(|e| e.to_string())?;
    drop(conn);
    progress.total.store(found.len(), Ordering::Relaxed);

    // A mutex per candidate: each worker takes its own index off the cursor, so
    // the locks only exist to hand `&mut` across the scope.
    {
        let cursor = AtomicUsize::new(0);
        let slots: Vec<Mutex<&mut Candidate>> = found.iter_mut().map(Mutex::new).collect();
        let workers = workers(slots.len());
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    loop {
                        if !progress.keep_going() {
                            break;
                        }
                        let Some(slot) = slots.get(cursor.fetch_add(1, Ordering::Relaxed)) else {
                            break;
                        };
                        bake::examine(&mut slot.lock().unwrap());
                        progress.done.fetch_add(1, Ordering::Relaxed);
                    }
                });
            }
        });
    }
    Ok(found)
}

#[derive(Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    wrote: AtomicUsize,
    /// Seeded with the survey's refusals; the run itself never adds to it.
    skipped: AtomicUsize,
    failed: AtomicUsize,
    current: Mutex<String>,
    cancel: AtomicBool,
    pace: rox_core::pace::Pace,
}

impl Progress {
    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    pub fn failed(&self) -> usize {
        self.failed.load(Ordering::Relaxed)
    }

    pub fn current(&self) -> String {
        self.current.lock().unwrap().clone()
    }

    pub fn stopping(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn eta_secs(&self) -> Option<f64> {
        self.pace.eta_secs(self.done(), self.total())
    }

    fn keep_going(&self) -> bool {
        !self.cancel.load(Ordering::Relaxed)
    }
}

#[derive(Clone)]
pub struct Summary {
    pub updated: usize,
    pub skipped: usize,
    pub failed: usize,
    pub stopped: bool,
}

impl Summary {
    /// All three numbers, zeros included: the skips are what people want to know.
    pub fn line(&self) -> String {
        let files = rox_i18n::t!("bake-summary-files", count = self.updated).to_string();
        let mut line = if self.stopped {
            rox_i18n::t!("bake-summary-stopped", files = files)
        } else {
            rox_i18n::t!("bake-summary-updated", files = files)
        }
        .to_string();
        line.push_str(&rox_i18n::t!("bake-summary-skipped", count = self.skipped));
        line.push_str(&rox_i18n::t!("bake-summary-failed", count = self.failed));
        line
    }
}

/// App-global so it outlives the dialog that started it.
#[derive(Default)]
struct Running(Option<Arc<Progress>>);

impl Global for Running {}

#[derive(Default)]
struct Last(Option<Summary>);

impl Global for Last {}

#[derive(Default)]
struct LastFailure(Option<String>);

impl Global for LastFailure {}

pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

pub fn last(cx: &App) -> Option<Summary> {
    cx.try_global::<Last>().and_then(|l| l.0.clone())
}

pub fn last_failure(cx: &App) -> Option<String> {
    cx.try_global::<LastFailure>().and_then(|f| f.0.clone())
}

pub fn dismiss(cx: &mut App) {
    cx.set_global(Last(None));
    cx.set_global(LastFailure(None));
}

/// Stops after the current file. Each write is one file, so nothing is half done.
pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel.store(true, Ordering::Relaxed);
    }
}

/// Write `items`, then claim the written files so the watcher doesn't bounce
/// them back as outside edits. `skipped` is the survey's refusals, carried
/// through to the finished line.
pub fn start(library: Entity<Library>, items: Vec<Item>, skipped: usize, cx: &mut App) {
    if progress(cx).is_some() || items.is_empty() {
        return;
    }
    let progress = Arc::new(Progress::default());
    progress.total.store(items.len(), Ordering::Relaxed);
    progress.skipped.store(skipped, Ordering::Relaxed);
    cx.set_global(Running(Some(progress.clone())));
    cx.set_global(Last(None));
    cx.set_global(LastFailure(None));
    // Nothing observes an app-global job on its own.
    crate::tasks_window::repaint_while_running(cx);
    crate::tasks_window::open(cx);
    // Quit raises the stop flag so a commit isn't cut off mid-file.
    cx.on_app_quit({
        let progress = progress.clone();
        move |_| {
            progress.cancel.store(true, Ordering::Relaxed);
            async {}
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        let (written, failure) = cx
            .background_executor()
            .spawn({
                let progress = progress.clone();
                async move { run(&items, &progress) }
            })
            .await;
        cx.update(|cx| {
            cx.set_global(Running(None));
            cx.set_global(Last(Some(Summary {
                updated: progress.wrote.load(Ordering::Relaxed),
                skipped: progress.skipped.load(Ordering::Relaxed),
                failed: progress.failed(),
                stopped: progress.stopping(),
            })));
            if let Some(failure) = failure {
                log::warn!("bake: {failure}");
                cx.set_global(LastFailure(Some(failure)));
            }
            library.update(cx, |library, cx| library.reindex_written(written, cx));
        })
        .ok();
    })
    .detach();
}

/// Returns the changed files and the first failure; they're usually all the same.
fn run(items: &[Item], progress: &Progress) -> (Vec<PathBuf>, Option<String>) {
    progress.pace.begin();
    let cursor = AtomicUsize::new(0);
    let written = Mutex::new(Vec::new());
    let failure: Mutex<Option<String>> = Mutex::new(None);
    let workers = workers(items.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    if !progress.keep_going() {
                        break;
                    }
                    let Some(item) = items.get(cursor.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    *progress.current.lock().unwrap() = item.path.to_string_lossy().into_owned();
                    match bake::apply(item) {
                        Ok(()) => {
                            written.lock().unwrap().push(item.path.clone());
                            progress.wrote.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => {
                            log::warn!("bake: {}: {e}", item.path.display());
                            progress.failed.fetch_add(1, Ordering::Relaxed);
                            let mut failure = failure.lock().unwrap();
                            if failure.is_none() {
                                *failure = Some(e);
                            }
                        }
                    }
                    progress.done.fetch_add(1, Ordering::Relaxed);
                }
            });
        }
    });
    (written.into_inner().unwrap(), failure.into_inner().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_finished_line_accounts_for_every_file() {
        assert_eq!(
            Summary {
                updated: 12,
                skipped: 3,
                failed: 0,
                stopped: false,
            }
            .line(),
            "12 files updated, 3 skipped, 0 failed"
        );
        assert_eq!(
            Summary {
                updated: 1,
                skipped: 0,
                failed: 2,
                stopped: true,
            }
            .line(),
            "Stopped after 1 file updated, 0 skipped, 2 failed"
        );
    }

    #[test]
    fn the_pool_stays_small_and_never_outgrows_the_work() {
        assert!(workers(1000) <= MAX_WORKERS);
        assert_eq!(workers(1), 1);
        assert_eq!(workers(0), 1);
    }
}
