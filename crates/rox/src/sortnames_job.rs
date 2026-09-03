//! The sort-name fill: ask MusicBrainz what each artist files under, and
//! put the answer in the library's own table.
//!
//! Almost nobody's files carry `ARTISTSORT`. Andrew's library has it on 24
//! of 53,343, so the columns, the letter rails and the search that
//! [`rox_library::projection`] now keys off a sort name have, for nearly
//! every artist, no sort name to key off. This pass is where the data
//! comes from: one lookup per artist, the answer stored in
//! [`rox_library::artist_meta`], the projection reloaded at the end so the
//! rails move.
//!
//! Nothing here writes a file. That's what makes a bulk run of it
//! legitimate under ADR 14, which rules out auto-applying a best guess
//! into a file's tags and sends every file write through a confirmed
//! picker. What this writes is rox's own opinion about a value, exactly
//! the shape the genre alias table already has, and a wrong row is undone
//! by deleting it rather than by rewriting an audio file.
//!
//! One worker, unlike the other three passes. MusicBrainz allows one
//! request a second and the module-level throttle in
//! [`rox_net::providers::musicbrainz`] holds the whole process to it, so a
//! second worker would spend its life asleep in that mutex. The rate limit
//! is also the pace: there's nothing to measure on this machine, which is
//! why the prompt can price the pass without a probe.
//!
//! The work list comes off the projection rather than out of SQL, because
//! the projection is the one place that already knows which artists have
//! no sort name from *either* source: it merged the file tags and the
//! table together to build the tables the health tile counts. Two
//! definitions of "unsorted artist" would drift, and the one the user is
//! looking at should win.

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{App, Entity, Global};

use rox_library::projection::{Projection, SymTable};
use rox_library::{artist_meta, store};
use rox_net::providers::musicbrainz::LookupError;
use rox_services::catalog::Library;

/// Worker-seconds an artist costs: the service's one-a-second rate limit
/// plus a request that takes a couple of hundred milliseconds. A constant
/// rather than a measured pace, the one pass where that's honest, because
/// the number is set by MusicBrainz rather than by this machine.
pub const PACE: f32 = 1.3;

/// Wire failures in a row before the pass gives up. A network that's gone
/// answers every artist the same way, and grinding through six thousand of
/// them at a second each to store nothing helps nobody. Generous enough
/// that a handful of scattered timeouts don't end a run that's working.
const GIVE_UP_AFTER: usize = 10;

/// Which artists the pass reaches.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Scope {
    /// Only names that aren't already Latin, which is the case the whole
    /// feature exists for and about a tenth of the work. A Latin name
    /// files close enough to right without a lookup; a CJK one doesn't
    /// file at all.
    #[default]
    NonLatin,
    /// Every artist with no sort name, Latin ones included. What this buys
    /// is the inverted form ("Yonezu, Kenshi", "Beatles, The") on names
    /// that currently file under their first word.
    All,
}

/// Whether a name already files where a Latin reader would look for it.
///
/// The fold is the test: it lowercases and strips diacritics, so anything
/// written in the Latin alphabet, accents and all, comes out ASCII. What
/// doesn't is the CJK, Cyrillic and Greek that lands in its own bucket at
/// the end of every rail.
pub fn is_latin(name: &str) -> bool {
    rox_library::fold::fold(name).is_ascii()
}

/// Every artist the pass would look up, in the order it would ask.
///
/// Both artist tables, since a lookup answers for the value rather than
/// for the column it appeared in, and one row fills it wherever it
/// appears. Deduplicated across the two: an artist who is also an album
/// artist is one request.
pub fn backlog(projection: &Projection, scope: Scope) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for table in [&projection.artists, &projection.album_artists] {
        collect(table, scope, &mut seen, &mut out);
    }
    out
}

/// One symbol table's share of the backlog: the values with no sort name
/// from either source, minus the ones already asked for and the ones the
/// scope leaves out.
fn collect(table: &SymTable, scope: Scope, seen: &mut HashSet<String>, out: &mut Vec<String>) {
    for sym in 0..table.strings.len() {
        let name = &table.strings[sym];
        // A nameless artist has no sort name and never will.
        if name.is_empty() {
            continue;
        }
        // Marked seen before anything else is asked about it, so a value
        // that answers in one table is settled for both. [`coverage`]
        // dedupes the same way, which is what keeps the count the prompt
        // shows and the list the pass works through the same number.
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

/// Where the library stands on artist sort names, for the rows and tiles
/// that say so before anything runs.
///
/// Counted in values rather than tracks, because a sort name belongs to
/// the artist: one lookup files every row they appear on.
#[derive(Clone, Copy, Default)]
pub struct Coverage {
    /// Artists with no sort name from either source, the pass's whole
    /// backlog.
    pub missing: u64,
    /// Named artists in the library, what `missing` is out of.
    pub total: u64,
    /// The share of `missing` a Latin reader can't file at all, which is
    /// what the default scope reaches.
    pub non_latin: u64,
}

/// Walk both artist tables for [`Coverage`]. None while no projection is
/// loaded, which reads as an empty library rather than as an error: there
/// is nothing to fill until there's something to fill it for.
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

/// Live progress of a fill: the worker writes it per artist, the UI polls
/// it. Zero total means the work list is still being built.
#[derive(Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    /// Artists MusicBrainz had no confident answer for. Nothing is stored
    /// for them, so they come back on the next run; the count is here so
    /// the readout can own up to a pass that wrote less than it asked
    /// about.
    failed: AtomicUsize,
    /// The artist under the cursor.
    current: Mutex<String>,
    /// Raised by [`stop`] and by app quit; the worker drops out after the
    /// request in flight.
    cancel: AtomicBool,
    /// The pass's clock, for the "about 10 minutes left" half of the
    /// readout. Started once the work list is built.
    pace: rox_core::pace::Pace,
}

impl Progress {
    /// Artists asked about so far.
    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    /// Artists the pass set out to ask about.
    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    /// Artists that came back without a sort name.
    pub fn failed(&self) -> usize {
        self.failed.load(Ordering::Relaxed)
    }

    /// The artist under the cursor.
    pub fn current(&self) -> String {
        self.current.lock().unwrap().clone()
    }

    /// Whether a stop has been asked for and the pass is winding down.
    pub fn stopping(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Seconds the rest of the pass should take at the rate so far.
    ///
    /// No `secs_per_track` beside it, unlike the other three passes:
    /// nothing here persists a measured pace, because the pace is
    /// MusicBrainz's rate limit and this machine has no say in it.
    pub fn eta_secs(&self) -> Option<f64> {
        self.pace.eta_secs(self.done(), self.total())
    }

    fn keep_going(&self) -> bool {
        !self.cancel.load(Ordering::Relaxed)
    }
}

/// The running pass, or nothing. App-global so it outlives the window that
/// started it, the shape the other three passes have.
#[derive(Default)]
struct Running(Option<Arc<Progress>>);

impl Global for Running {}

/// The running pass's progress, for any UI that shows it. None when
/// nothing is filling.
pub fn progress(cx: &App) -> Option<Arc<Progress>> {
    cx.try_global::<Running>().and_then(|r| r.0.clone())
}

/// Signal the running pass to stop. What it already stored stays; a no-op
/// when nothing is running.
pub fn stop(cx: &mut App) {
    if let Some(progress) = progress(cx) {
        progress.cancel.store(true, Ordering::Relaxed);
    }
}

/// Look up every artist in `scope` that has no sort name, and store what
/// came back. A no-op while a pass is already running.
///
/// Safe to call from inside the library's own update: the work list is
/// read in the spawned task, by which time the lease is gone.
pub fn start(library: Entity<Library>, scope: Scope, cx: &mut App) {
    if progress(cx).is_some() {
        return;
    }
    let progress = Arc::new(Progress::default());
    cx.set_global(Running(Some(progress.clone())));
    // Keeps the menubar chip and the tasks window ticking; nothing
    // observes an app-global pass on its own.
    crate::tasks_window::repaint_while_running(cx);
    // Quitting mid-pass raises the same flag the stop button does, so the
    // worker stops between artists rather than being killed mid-write.
    cx.on_app_quit({
        let progress = progress.clone();
        move |_| {
            progress.cancel.store(true, Ordering::Relaxed);
            async {}
        }
    })
    .detach();
    cx.spawn(async move |cx| {
        // The projection and the database path both come off the library,
        // read here rather than up top so a caller inside its own update
        // is safe. The read only fails with the app already on its way
        // out.
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
                    // The rows went into rox's own table, which the
                    // projection lays over the symbol tables as it
                    // builds, so a reload is what actually moves the
                    // letter rails and teaches search the Latin
                    // spellings.
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

/// The blocking half: one artist at a time, ask, store what came back.
/// Returns how many artists took a sort name.
///
/// Serial on purpose; see the module header. A `None` stores nothing, so
/// the artist is asked about again on the next run, which is the right
/// answer for a MusicBrainz entry that gains a sort name later.
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
        // Handed to the lookup so a stop click lands while it's waiting
        // out a busy server's Retry-After, instead of only between names.
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
            // A busy server is MusicBrainz shedding load, retried already
            // inside the lookup. It says nothing about whether the next
            // name will go through, so it isn't counted toward giving up:
            // the artist is simply asked again on the next run.
            Err(LookupError::Busy) => {
                log::warn!("sortnames: {name}: service busy, skipped");
                progress.failed.fetch_add(1, Ordering::Relaxed);
            }
            // The stop came in mid-wait, so the name was never really
            // asked about. Not counted as a failure and not counted as
            // done: the loop is finished either way.
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

    /// A table built by hand rather than through a projection: the fill is
    /// a question about symbols and their sort names, and a scratch
    /// database would only be a slower way to write these four rows.
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
        // An accented Latin name folds to ASCII, so it files where it
        // should already and the default pass leaves it alone.
        assert_eq!(names(&table, Scope::NonLatin), ["米津玄師"]);
        // The wider scope adds the Latin names, still skipping the one
        // that already has a sort name and the empty value.
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
