//! Embedding stored metadata: the run behind [`crate::bake_dialog`].
//!
//! [`rox_library::bake`] decides what a bake is; this is the machinery around
//! it. Two blocking halves, both on the background executor: the survey, which
//! is a database read and then a tag read per candidate, and the run, which is
//! a commit per file.
//!
//! The run copies [`crate::convert`]'s shape - an app-global `Arc<Progress>`
//! the tasks window polls, a summary and a first-failure line that outlive it
//! for the row that reports on them - because it's the same kind of job: short,
//! started from a dialog that closes on the press, and worth saying something
//! about afterwards. The pool is convert's too, and for a related reason:
//! nothing here decodes, but every write is a clone-verify-rename of a whole
//! file, so this is disk rather than CPU and going wide on a spinning disk
//! makes it slower rather than faster.
//!
//! Nothing is computed anywhere in here. Every value written was already in
//! the database, the lyrics store or a sidecar, which is what makes the whole
//! thing safe to run twice: the second pass finds the tags there and skips.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{App, Entity, Global};

use rox_library::bake::{self, Candidate, Item};
use rox_library::store;
use rox_services::catalog::Library;

/// The most files worked on at once. Convert's ceiling, since a tag commit is
/// a whole-file copy and four of those already own a disk.
const MAX_WORKERS: usize = 4;

/// Half the cores, capped, and never more than there is work for.
fn workers(len: usize) -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get() / 2)
        .unwrap_or(1)
        .clamp(1, MAX_WORKERS)
        .min(len.max(1))
}

/// How far along the survey is, for a dialog that would otherwise sit blank
/// while a described library's tags are read.
#[derive(Default)]
pub struct Survey {
    done: AtomicUsize,
    total: AtomicUsize,
    cancel: AtomicBool,
}

impl Survey {
    /// Candidates whose files have been looked at.
    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    /// Candidates there are to look at. Zero while the database half is still
    /// running, which is the state a big library sits in for a moment.
    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    /// Give up: the dialog closed and nobody is waiting for the answer.
    pub fn abandon(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn keep_going(&self) -> bool {
        !self.cancel.load(Ordering::Relaxed)
    }
}

/// Everything a bake could write, refusals and all. Blocking and potentially
/// long: the tag reads are one file open each, so this runs on the background
/// executor with `progress` shared out to whatever is drawing it.
///
/// A cancelled survey comes back with what it had, which the caller throws
/// away - the counts would be wrong, and the only cancel is the dialog
/// closing.
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

    // Only the ones nothing has refused yet cost a file open, so a library
    // full of formats the writer can't reach surveys almost instantly.
    //
    // A mutex per candidate rather than one over the list: every worker takes
    // its own index off the cursor, so no two ever reach for the same slot and
    // the locks are the price of handing `&mut` across a scope at all.
    {
        let cursor = AtomicUsize::new(0);
        let slots: Vec<Mutex<&mut Candidate>> = found.iter_mut().map(Mutex::new).collect();
        let workers = workers(slots.len());
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| loop {
                    if !progress.keep_going() {
                        break;
                    }
                    let Some(slot) = slots.get(cursor.fetch_add(1, Ordering::Relaxed)) else {
                        break;
                    };
                    bake::examine(&mut slot.lock().unwrap());
                    progress.done.fetch_add(1, Ordering::Relaxed);
                });
            }
        });
    }
    Ok(found)
}

/// Live progress of a run: a worker writes it per file, the tasks window
/// polls it.
#[derive(Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    /// Files that took their tags.
    wrote: AtomicUsize,
    /// Files nothing was written for. Seeded with what the survey refused
    /// before the run began; nothing is added here, since every item that
    /// became work has something to write.
    skipped: AtomicUsize,
    /// Files the writer would not commit to.
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

/// What a run left behind, for the row that reports on it afterwards.
#[derive(Clone)]
pub struct Summary {
    pub updated: usize,
    pub skipped: usize,
    pub failed: usize,
    pub stopped: bool,
}

impl Summary {
    /// The one-line report. All three numbers, zeros included: the skips are
    /// most of what someone wants to know afterwards, and "0 failed" is the
    /// answer to the question a count of updates raises.
    pub fn line(&self) -> String {
        let files = if self.updated == 1 {
            "1 file".to_string()
        } else {
            format!("{} files", self.updated)
        };
        let head = if self.stopped {
            format!("Stopped after {files} updated")
        } else {
            format!("{files} updated")
        };
        format!("{head}, {} skipped, {} failed", self.skipped, self.failed)
    }
}

/// The running bake, or nothing. App-global so it outlives the dialog that
/// started it.
#[derive(Default)]
struct Running(Option<Arc<Progress>>);

impl Global for Running {}

/// The last run's report, kept for the tasks window until it's dismissed.
#[derive(Default)]
struct Last(Option<Summary>);

impl Global for Last {}

/// Why the first file that failed did, kept beside the summary: a count with
/// no reason is what sends someone to the log.
#[derive(Default)]
struct LastFailure(Option<String>);

impl Global for LastFailure {}

/// The running bake's progress, for a UI that wants to show it.
pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

/// How the last run went. None until one has run this session, and None again
/// once its row has been dismissed.
pub fn last(cx: &App) -> Option<Summary> {
    cx.try_global::<Last>().and_then(|l| l.0.clone())
}

/// What the writer said about the last file that failed, if one did.
pub fn last_failure(cx: &App) -> Option<String> {
    cx.try_global::<LastFailure>().and_then(|f| f.0.clone())
}

/// Drop the last run's report, the X on its row.
pub fn dismiss(cx: &mut App) {
    cx.set_global(Last(None));
    cx.set_global(LastFailure(None));
}

/// Ask the running bake to stop after the file it's on. What it already
/// wrote stays: every write is one file's tags reaching their own file, so
/// there's nothing half-done to undo.
pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel.store(true, Ordering::Relaxed);
    }
}

/// Write `items`, and hand every file that took a write to the library so the
/// watcher doesn't bounce it back as an outside edit.
///
/// `skipped` is what the survey refused for the picked sources, carried
/// through so the finished line can account for every file the dialog counted.
/// A no-op while a run is already going.
pub fn start(library: Entity<Library>, items: Vec<Item>, skipped: usize, cx: &mut App) {
    if progress(cx).is_some() || items.is_empty() {
        return;
    }
    let progress = Arc::new(Progress::default());
    progress.total.store(items.len(), Ordering::Relaxed);
    progress.skipped.store(skipped, Ordering::Relaxed);
    cx.set_global(Running(Some(progress.clone())));
    // A fresh run's report replaces the last one rather than sitting under
    // it, so the row never shows an old count beside a live bar.
    cx.set_global(Last(None));
    cx.set_global(LastFailure(None));
    // Nothing observes an app-global job on its own; this is what keeps the
    // tasks window and the menubar chip ticking while it runs.
    crate::tasks_window::repaint_while_running(cx);
    // The run outlives the dialog, which closes on the press, so hand over
    // something that carries the count and the stop button.
    crate::tasks_window::open(cx);
    // Quitting mid-run shouldn't leave a commit half done. The flag the stop
    // button raises goes up on the way out too; a worker is between files
    // within one file's write.
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
            // Every path that changed on disk, so the watcher drops its own
            // event and the rows pick the new tags up. The same handoff the
            // ReplayGain and acoustic passes end on.
            library.update(cx, |library, cx| library.reindex_written(written, cx));
        })
        .ok();
    })
    .detach();
}

/// The blocking half: a bounded pool over a cursor, convert's shape. Returns
/// the files that changed and the first failure's reason, since one reason is
/// what a row can show and they're usually all the same reason.
fn run(items: &[Item], progress: &Progress) -> (Vec<PathBuf>, Option<String>) {
    progress.pace.begin();
    let cursor = AtomicUsize::new(0);
    let written = Mutex::new(Vec::new());
    let failure: Mutex<Option<String>> = Mutex::new(None);
    let workers = workers(items.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
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
                    // One file that won't take a tag costs its own tags and
                    // nothing else: the values are still in the database, and
                    // the next file is unaffected.
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
            });
        }
    });
    (written.into_inner().unwrap(), failure.into_inner().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The report reads as a sentence whichever way the run went, and never
    /// leaves a number out: the skips are the interesting half, and they're
    /// the ones a count of updates would otherwise hide.
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

    /// A modest pool: this is disk rather than CPU, and a machine with
    /// thirty-two cores must not point all of them at one drive.
    #[test]
    fn the_pool_stays_small_and_never_outgrows_the_work() {
        assert!(workers(1000) <= MAX_WORKERS);
        assert_eq!(workers(1), 1);
        assert_eq!(workers(0), 1);
    }
}
