//! The sort-name fill: ask MusicBrainz what each artist files under, and store
//! the answer in [`rox_library::artist_meta`]. Almost no files carry
//! `ARTISTSORT` (24 of 53,343 in Andrew's library), so without this the letter
//! rails and sort-name search have nothing to key off.
//!
//! Nothing here writes a file, which is what makes a bulk run legitimate under
//! ADR 14: a wrong row is undone by deleting it.
//!
//! One worker: the throttle in [`rox_net::providers::musicbrainz`] holds the
//! process to MusicBrainz's one request a second, so the rate limit is also the
//! pace and the prompt needs no probe.
//!
//! The work list comes off the projection, the one place that merges file tags
//! and the table, so it matches what the health tile counts.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{App, Entity, Global};

use rox_library::projection::{Projection, SymTable};
use rox_library::{artist_meta, store};
use rox_net::providers::musicbrainz::LookupError;
use rox_services::catalog::Library;

/// One second of rate limit plus a request of a couple of hundred milliseconds.
/// A constant, since MusicBrainz sets it, not this machine.
pub const PACE: f32 = 1.3;

/// Wire failures in a row before giving up: a network that's gone answers every
/// artist the same way.
const GIVE_UP_AFTER: usize = 10;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Scope {
    /// About a tenth of the work: a CJK name doesn't file at all.
    #[default]
    NonLatin,
    /// Adds the inverted form ("Beatles, The") for names that file under their
    /// first word.
    All,
}

/// Latin script, accents and all, folds to ASCII.
pub fn is_latin(name: &str) -> bool {
    rox_library::fold::fold(name).is_ascii()
}

/// Both artist tables, deduplicated: the answer is for the value, not the
/// column.
pub fn backlog(projection: &Projection, scope: Scope) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for table in [&projection.artists, &projection.album_artists] {
        collect(table, scope, &mut seen, &mut out);
    }
    out
}

fn collect(table: &SymTable, scope: Scope, seen: &mut HashSet<String>, out: &mut Vec<String>) {
    for sym in 0..table.strings.len() {
        let name = &table.strings[sym];
        if name.is_empty() {
            continue;
        }
        // Marked before the sort-name check, like [`coverage`], so the prompt's
        // count matches the list the pass works through.
        if !seen.insert(name.clone()) {
            continue;
        }
        if !table.sort_name(sym).is_empty() {
            continue;
        }
        if scope == Scope::NonLatin && is_latin(name) {
            continue;
        }
        out.push(name.clone());
    }
}

/// Counted in values: one lookup files every row the artist appears on.
#[derive(Clone, Copy, Default)]
pub struct Coverage {
    pub missing: u64,
    pub total: u64,
    /// What the default scope reaches.
    pub non_latin: u64,
}

pub fn coverage(projection: Option<&Projection>) -> Coverage {
    let Some(projection) = projection else {
        return Coverage::default();
    };
    let mut seen = HashSet::new();
    let mut out = Coverage::default();
    for table in [&projection.artists, &projection.album_artists] {
        for sym in 0..table.strings.len() {
            let name = &table.strings[sym];
            if name.is_empty() || !seen.insert(name.as_str()) {
                continue;
            }
            out.total += 1;
            if table.sort_name(sym).is_empty() {
                out.missing += 1;
                if !is_latin(name) {
                    out.non_latin += 1;
                }
            }
        }
    }
    out
}

#[derive(Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    /// Nothing is stored for these, so they come back on the next run.
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

    /// No `secs_per_track`: the pace is MusicBrainz's rate limit, so nothing is
    /// persisted.
    pub fn eta_secs(&self) -> Option<f64> {
        self.pace.eta_secs(self.done(), self.total())
    }

    fn keep_going(&self) -> bool {
        !self.cancel.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
struct Running(Option<Arc<Progress>>);

impl Global for Running {}

pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel.store(true, Ordering::Relaxed);
    }
}

/// A no-op while a pass runs. Safe inside the library's own update: the work
/// list is read in the spawned task.
pub fn start(library: Entity<Library>, scope: Scope, cx: &mut App) {
    if progress(cx).is_some() {
        return;
    }
    let progress = Arc::new(Progress::default());
    cx.set_global(Running(Some(progress.clone())));
    crate::tasks_window::repaint_while_running(cx);
    cx.on_app_quit({
        let progress = progress.clone();
        move |_| {
            progress.cancel.store(true, Ordering::Relaxed);
            async {}
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        let Ok((db_path, work)) = cx.update(|cx| {
            let library = library.read(cx);
            (
                library.db_path(),
                library
                    .projection()
                    .map(|projection| backlog(projection, scope))
                    .unwrap_or_default(),
            )
        }) else {
            return;
        };
        let written = cx
            .background_executor()
            .spawn({
                let progress = progress.clone();
                async move { run(&db_path, work, &progress) }
            })
            .await;
        cx.update(|cx| {
            cx.set_global(Running(None));
            match written {
                Ok(0) => {}
                Ok(written) => {
                    log::info!("sortnames: {written} artists filled");
                    library.update(cx, |library, cx| library.reload_projection(cx));
                }
                Err(e) => {
                    log::error!("sortnames: {e}");
                }
            }
        })
        .ok();
    })
    .detach();
}

/// A `None` stores nothing, so the artist is asked again next run.
fn run(db_path: &Path, work: Vec<String>, progress: &Progress) -> Result<usize, String> {
    let conn = store::open(db_path).map_err(|e| e.to_string())?;
    progress.total.store(work.len(), Ordering::Relaxed);
    progress.pace.begin();

    let mut written = 0usize;
    let mut consecutive_errors = 0usize;
    for name in work {
        if !progress.keep_going() {
            break;
        }
        *progress.current.lock().unwrap() = name.clone();
        // Lets a stop land while the lookup waits out a busy server's
        // Retry-After.
        let cancel = || !progress.keep_going();
        match rox_net::providers::musicbrainz::artist_sort_name(&name, Some(&cancel)) {
            Ok(Some(sort)) => {
                consecutive_errors = 0;
                artist_meta::set(&conn, &name, &sort, artist_meta::MUSICBRAINZ)
                    .map_err(|e| e.to_string())?;
                written += 1;
            }
            Ok(None) => {
                consecutive_errors = 0;
                log::debug!("sortnames: no answer for {name}");
                progress.failed.fetch_add(1, Ordering::Relaxed);
            }
            // Retried inside the lookup already, and says nothing about the
            // next name, so it doesn't count toward giving up.
            Err(LookupError::Busy) => {
                log::warn!("sortnames: {name}: service busy, skipped");
                progress.failed.fetch_add(1, Ordering::Relaxed);
            }
            // Stopped mid-wait: neither a failure nor done.
            Err(LookupError::Cancelled) => break,
            Err(e) => {
                log::warn!("sortnames: {name}: {e}");
                progress.failed.fetch_add(1, Ordering::Relaxed);
                consecutive_errors += 1;
                if consecutive_errors >= GIVE_UP_AFTER {
                    return Err(format!("gave up after {consecutive_errors} failures: {e}"));
                }
            }
        }
        progress.done.fetch_add(1, Ordering::Relaxed);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(rows: &[(&str, &str)]) -> SymTable {
        let strings: Vec<String> = rows.iter().map(|(name, _)| name.to_string()).collect();
        let sort: Vec<String> = rows.iter().map(|(_, sort)| sort.to_string()).collect();
        let lower = strings.iter().map(|s| rox_library::fold::fold(s)).collect();
        let sort_lower = sort.iter().map(|s| rox_library::fold::fold(s)).collect();
        SymTable {
            strings,
            lower,
            sort,
            sort_lower,
        }
    }

    fn names(table: &SymTable, scope: Scope) -> Vec<String> {
        let mut out = Vec::new();
        collect(table, scope, &mut HashSet::new(), &mut out);
        out
    }

    #[test]
    fn the_default_scope_only_reaches_names_a_latin_reader_cant_file() {
        let table = table(&[
            ("米津玄師", ""),
            ("Beyoncé", ""),
            ("Zebra", ""),
            ("崎山蒼志", "Sakiyama, Soushi"),
            ("", ""),
        ]);
        // An accented Latin name already files right, so the default scope
        // skips it.
        assert_eq!(names(&table, Scope::NonLatin), ["米津玄師"]);
        assert_eq!(names(&table, Scope::All), ["米津玄師", "Beyoncé", "Zebra"]);
    }

    #[test]
    fn an_artist_in_both_tables_is_asked_about_once() {
        let artists = table(&[("米津玄師", ""), ("崎山蒼志", "")]);
        let album_artists = table(&[("米津玄師", ""), ("서태지", "")]);
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        collect(&artists, Scope::NonLatin, &mut seen, &mut out);
        collect(&album_artists, Scope::NonLatin, &mut seen, &mut out);
        assert_eq!(out, ["米津玄師", "崎山蒼志", "서태지"]);
    }

    #[test]
    fn latin_is_what_the_fold_can_flatten() {
        assert!(is_latin("Beyoncé"));
        assert!(is_latin("Straße"));
        assert!(is_latin("AC/DC"));
        assert!(!is_latin("米津玄師"));
        assert!(!is_latin("Мумий Тролль"));
    }
}
