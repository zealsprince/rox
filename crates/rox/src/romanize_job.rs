//! The romanization pass: read every non-Latin title, album and artist the
//! library still has no sort name for, and write down what it says in
//! Latin letters.
//!
//! [`crate::sortnames_job`] filled the artist half from MusicBrainz. This
//! is the rest of it, and it exists because there was nowhere left to ask:
//! files don't carry sort titles, MusicBrainz has no sort name for a
//! release and none at all for a track, so a library of Japanese titles
//! had one remaining source, which is to read the characters.
//! [`rox_romanize`] does the reading; this walks the library, decides what
//! to hand it, and puts the answers in the three meta tables.
//!
//! Nothing here writes a file, and nothing here talks to a service. That
//! makes it the cheapest of the five passes by a wide margin, and it's
//! also why it can be run again after installing a dictionary without
//! costing anything: rows that answered keep their answers, rows that
//! didn't get another look.
//!
//! ## Deduplicated by value, applied by row
//!
//! An artist appears on hundreds of rows and their name reads the same on
//! every one, so the work list is distinct values rather than rows. Titles
//! are the exception in one direction: the value is still deduplicated
//! (two tracks called レモン are one romanization), but the answer is
//! written per track id, because a title has no symbol in the projection to
//! hang a sort name off.
//!
//! ## Which language a kanji title is in
//!
//! Kanji and hanzi are the same characters. 東京 is `toukyou` or
//! `dongjing` depending on nothing you can see in the string. The pass
//! settles it with the row: a value that appears anywhere alongside kana
//! is Japanese, everything else Han-only is read as Mandarin. That's the
//! one thing [`rox_romanize`] can't work out for itself and the reason it
//! takes a [`Reading`] at all.
//!
//! ## The dictionary
//!
//! Kana, hangul and Mandarin need nothing. Kanji needs the IPADIC download
//! ([`rox_romanize::dictionary`]), and without it those values are skipped
//! and everything else still runs. The pass used to refuse to start over
//! them, which was wrong twice over: most libraries have far more
//! non-kanji values than kanji ones, and a dimmed button over work that
//! would have gone through fine reads as broken. The prompt counts the
//! kanji up front and says how many will be left, the tasks window
//! reports how many were, and installing the dictionary and running again
//! picks up exactly those.

pub mod dictionary;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{App, Entity, Global};

use rox_library::projection::{Projection, SymTable};
use rox_library::{album_meta, artist_meta, store, track_meta};
use rox_romanize::{Japanese, Reading};
use rox_services::catalog::Library;

/// Values written inside one transaction. The pass is IO-bound on nothing
/// but these writes, and a transaction per row would fsync a thousand
/// times where one will do; a transaction over the whole backlog would
/// hold a write lock for the length of the pass and lose everything to a
/// stop. A thousand is the compromise, and it's the same order the scanner
/// batches its inserts at.
const BATCH: usize = 1_000;

/// How many values the pace probe reads before it decides how long the
/// rest will take. Enough to average out one slow lookup, few enough that
/// the probe is over before the dialog has finished drawing.
const SAMPLE: usize = 100;

/// Where a romanized value's answer goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A track title, written per track id because titles aren't interned.
    /// Every row that carries this exact title and no sort title of its
    /// own.
    Title(Vec<i64>),
    /// An album title, one row in [`album_meta`] for the value.
    Album,
    /// An artist or album artist, one row in [`artist_meta`] for the value.
    /// Both tables read the same row, the way the sort-name fill's answers
    /// do.
    Artist,
}

/// One value the pass would romanize.
#[derive(Clone, Debug)]
pub struct Item {
    pub value: String,
    /// What language its Han characters are in, worked out from the rows
    /// the value appears on.
    pub reading: Reading,
    pub target: Target,
}

/// Everything the pass would do, and what it needs to do it.
#[derive(Default)]
pub struct Backlog {
    pub items: Vec<Item>,
    /// Distinct named values the walk looked at, filled or not. What
    /// `items.len()` is out of.
    pub considered: u64,
}

impl Backlog {
    /// How many of its values are kanji, and so need the downloaded
    /// dictionary. Counted before the pass starts, because it's the number
    /// the prompt quotes and the number that gets skipped when the
    /// download isn't installed.
    pub fn kanji(&self) -> u64 {
        self.items
            .iter()
            .filter(|item| rox_romanize::needs_dictionary(&item.value, item.reading))
            .count() as u64
    }
}

/// Whether a value already files where a Latin reader would look for it.
/// The same fold test [`crate::sortnames_job::is_latin`] uses, and for the
/// same reason: anything written in the Latin alphabet folds to ASCII, and
/// what doesn't is what this pass is for.
fn worth_reading(value: &str) -> bool {
    !value.is_empty() && !rox_library::fold::fold(value).is_ascii()
}

/// The rows an older spelling of this pass wrote, which the next run reads
/// again: the projection shows them as filled, and only the tables know
/// their reading came from a build that joined words differently. Loaded
/// once per prompt or count, three small selects.
#[derive(Default, Clone, Debug)]
pub struct Stale {
    pub titles: HashSet<i64>,
    pub albums: HashSet<String>,
    pub artists: HashSet<String>,
}

/// What the pass wrote under an older marker than this build's. An
/// unreadable database reads as nothing stale, and says so in the log,
/// rather than as a reason not to count.
pub fn stale(db_path: &Path) -> Stale {
    let marker = artist_meta::romanized_marker(rox_romanize::VERSION);
    let load = || -> rox_library::rusqlite::Result<Stale> {
        let conn = store::open(db_path)?;
        Ok(Stale {
            titles: track_meta::stale_romanized(&conn, &marker)?,
            albums: album_meta::stale_romanized(&conn, &marker)?,
            artists: artist_meta::stale_romanized(&conn, &marker)?,
        })
    };
    load().unwrap_or_else(|e| {
        log::warn!("romanize: stale rows unreadable, counting none: {e}");
        Stale::default()
    })
}

/// Every value the pass would romanize, in the order it would read them.
///
/// One walk of the rows and one of each symbol table. The row walk does
/// two jobs: it collects the titles, which live per row, and it works out
/// which values appear beside kana, which is how a bare-kanji value gets
/// its language. A value in `stale` counts as unfilled however the
/// projection shows it.
pub fn backlog(projection: &Projection, stale: &Stale) -> Backlog {
    // One set per interner. The three tables number their symbols
    // independently, so symbol 7 in `artists` and symbol 7 in
    // `album_artists` are unrelated values and pooling them marks the
    // wrong names Japanese.
    let mut japanese_artists: HashSet<u32> = HashSet::new();
    let mut japanese_album_artists: HashSet<u32> = HashSet::new();
    let mut japanese_albums: HashSet<u32> = HashSet::new();
    // Title values seen anywhere beside kana. Kept by value rather than
    // settled as the walk goes, because the row that proves a title is
    // Japanese can be one this pass skips (it already has a sort title)
    // and can come after the row that put the title in the work list.
    let mut japanese_titles: HashSet<&str> = HashSet::new();
    // Title value -> its index in `titles`, so two tracks with the same
    // title are one romanization applied to both rows.
    let mut seen_titles: HashMap<&str, usize> = HashMap::new();
    let mut titles: Vec<Item> = Vec::new();
    let mut considered: u64 = 0;
    let mut title_values: HashSet<&str> = HashSet::new();

    for row in 0..projection.len() {
        if projection.is_dead(row as u32) {
            continue;
        }
        let title = projection.title.get(row);
        let artist = projection.artist[row];
        let album_artist = projection.album_artist[row];
        let album = projection.album[row];
        // Kana anywhere on the row settles the language for everything on
        // it. Cheap: most rows have none and the test stops at the first
        // character that isn't one.
        let reads_japanese = rox_romanize::has_kana(title)
            || rox_romanize::has_kana(&projection.artists.strings[artist as usize])
            || rox_romanize::has_kana(&projection.album_artists.strings[album_artist as usize])
            || rox_romanize::has_kana(&projection.albums.strings[album as usize]);
        if reads_japanese {
            japanese_artists.insert(artist);
            japanese_album_artists.insert(album_artist);
            japanese_albums.insert(album);
            japanese_titles.insert(title);
        }
        if !title.is_empty() && title_values.insert(title) {
            considered += 1;
        }
        // A row that already has a sort title, from its own tags or from an
        // earlier run of this same spelling, is settled. One an older
        // spelling wrote is not.
        let settled = !projection.title_sort(row).is_empty()
            && !stale.titles.contains(&projection.db_id[row]);
        if settled || !worth_reading(title) {
            continue;
        }
        match seen_titles.get(title) {
            Some(&at) => {
                if let Target::Title(rows) = &mut titles[at].target {
                    rows.push(projection.db_id[row]);
                }
            }
            None => {
                seen_titles.insert(title, titles.len());
                titles.push(Item {
                    value: title.to_string(),
                    // Provisional: any row carrying this title beside
                    // kana promotes it once the walk is done.
                    reading: Reading::Auto,
                    target: Target::Title(vec![projection.db_id[row]]),
                });
            }
        }
    }
    for item in &mut titles {
        if japanese_titles.contains(item.value.as_str()) {
            item.reading = Reading::Japanese;
        }
    }

    let mut items = titles;
    // Both artist tables feed one list of values: a romanization answers
    // for the value rather than for the column it turned up in, and
    // artist_meta is read by both tables when the projection loads.
    let mut seen = HashSet::new();
    for (table, japanese) in [
        (&projection.artists, &japanese_artists),
        (&projection.album_artists, &japanese_album_artists),
    ] {
        collect(
            table,
            japanese,
            Target::Artist,
            &stale.artists,
            &mut seen,
            &mut items,
            &mut considered,
        );
    }
    collect(
        &projection.albums,
        &japanese_albums,
        Target::Album,
        &stale.albums,
        &mut HashSet::new(),
        &mut items,
        &mut considered,
    );
    Backlog { items, considered }
}

/// One symbol table's share of the backlog: the values with no sort name
/// from any source, minus the ones already asked about.
#[allow(clippy::too_many_arguments)]
fn collect(
    table: &SymTable,
    japanese: &HashSet<u32>,
    target: Target,
    stale: &HashSet<String>,
    seen: &mut HashSet<String>,
    out: &mut Vec<Item>,
    considered: &mut u64,
) {
    for sym in 0..table.strings.len() {
        let name = &table.strings[sym];
        if name.is_empty() {
            continue;
        }
        // Counted and marked before anything else is asked about it, so a
        // value settled in one table is settled for both and the count the
        // prompt shows matches the list the pass works through.
        if !seen.insert(name.clone()) {
            continue;
        }
        *considered += 1;
        let settled = !table.sort_name(sym).is_empty() && !stale.contains(name);
        if settled || !worth_reading(name) {
            continue;
        }
        out.push(Item {
            value: name.clone(),
            reading: if japanese.contains(&(sym as u32)) {
                Reading::Japanese
            } else {
                Reading::Auto
            },
            target: target.clone(),
        });
    }
}

/// Where the library stands on romanizable values, for the rows that say
/// so before anything runs.
#[derive(Clone, Copy, Default)]
pub struct Coverage {
    /// Values with no sort name and something to read, the whole backlog.
    pub missing: u64,
    /// Distinct named values in the library, what `missing` is out of.
    pub total: u64,
    /// How many of `missing` are kanji, and so need the download. These
    /// still run when it's installed and are skipped when it isn't;
    /// nothing about them stops the rest of the pass.
    pub kanji: u64,
}

/// Walk the library for [`Coverage`]. Zero while no projection is loaded,
/// which reads as an empty library rather than as an error.
pub fn coverage(projection: Option<&Projection>, stale: &Stale) -> Coverage {
    let Some(projection) = projection else {
        return Coverage::default();
    };
    let backlog = backlog(projection, stale);
    Coverage {
        missing: backlog.items.len() as u64,
        total: backlog.considered,
        kanji: backlog.kanji(),
    }
}

/// Whether the Japanese dictionary is installed. The pass's one hard
/// requirement, and only when the backlog holds kanji.
pub fn dictionary_installed() -> bool {
    rox_romanize::dictionary::IPADIC.installed()
}

/// Time the first hundred values so the prompt can price the rest.
///
/// A measured pace rather than a constant, unlike the sort-name fill: this
/// pass has no rate limit setting its speed, so what it costs is what this
/// machine costs, and that swings by an order of magnitude between a
/// library of kana (a table lookup per character) and one of kanji (a
/// Viterbi lattice per title).
///
/// Runs on the background executor; the sample comes off the projection on
/// the UI thread before it's called.
pub fn measure_pace(sample: Vec<Item>) -> Result<f32, String> {
    if sample.is_empty() {
        return Err("nothing left to romanize".to_string());
    }
    let ja = load_dictionary(&sample);
    let started = std::time::Instant::now();
    let mut read = 0usize;
    for item in sample.iter().take(SAMPLE) {
        let _ = rox_romanize::romanize_as(&item.value, ja, item.reading);
        read += 1;
    }
    Ok(started.elapsed().as_secs_f32() / read as f32)
}

/// The dictionary if the work needs it and it's there. None means either
/// that nothing in the work list is kanji or that the download is
/// missing, and the caller has already refused to start in the second case.
///
/// The load itself belongs to [`rox_romanize::japanese`], which keeps one
/// mapping per process: the metadata panel's Romanize button reads the
/// same dictionary this pass does, and forty megabytes is not a thing to
/// map twice.
fn load_dictionary(items: &[Item]) -> Option<&'static Japanese> {
    items
        .iter()
        .any(|item| rox_romanize::needs_dictionary(&item.value, item.reading))
        .then(rox_romanize::japanese)
        .flatten()
}

/// Live progress of a run: the worker writes it per value, the UI polls
/// it. Zero total means the work list is still being built.
#[derive(Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    /// Values nothing could be read out of: a script this doesn't cover,
    /// or kanji with no dictionary loaded. Nothing is stored for them, so
    /// they come back on the next run.
    failed: AtomicUsize,
    /// Values left alone because they're kanji and no dictionary was
    /// loaded. A subset of `failed`, counted apart because it's the one
    /// kind of miss a person can do something about: install the download
    /// and run again.
    skipped: AtomicUsize,
    /// The value under the cursor.
    current: Mutex<String>,
    /// Raised by [`stop`] and by app quit; the worker drops out between
    /// values.
    cancel: AtomicBool,
    /// The pass's clock, for the "about a minute left" half of the
    /// readout.
    pace: rox_core::pace::Pace,
}

impl Progress {
    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    /// Values that came back without a romanization.
    pub fn failed(&self) -> usize {
        self.failed.load(Ordering::Relaxed)
    }

    /// Values skipped for want of the Japanese dictionary.
    pub fn skipped(&self) -> usize {
        self.skipped.load(Ordering::Relaxed)
    }

    /// The value under the cursor.
    pub fn current(&self) -> String {
        self.current.lock().unwrap().clone()
    }

    pub fn stopping(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Seconds the rest of the pass should take at the rate so far.
    pub fn eta_secs(&self) -> Option<f64> {
        self.pace.eta_secs(self.done(), self.total())
    }

    /// Worker-seconds a value has cost so far, for the settings file so the
    /// next prompt opens with a number. None before there's enough of a
    /// run to average.
    pub fn secs_per_value(&self) -> Option<f32> {
        self.pace
            .secs_per_track(self.done())
            .map(|secs| secs as f32)
    }

    fn keep_going(&self) -> bool {
        !self.cancel.load(Ordering::Relaxed)
    }
}

/// The running pass, or nothing. App-global so it outlives the window that
/// started it, the shape the other four passes have.
#[derive(Default)]
struct Running(Option<Arc<Progress>>);

impl Global for Running {}

/// The running pass's progress, for any UI that shows it. None when
/// nothing is running.
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

/// Romanize every value with no sort name and store what came back. A
/// no-op while a pass is already running.
///
/// Safe to call from inside the library's own update: the work list is
/// read in the spawned task, by which time the lease is gone.
pub fn start(library: Entity<Library>, cx: &mut App) {
    if progress(cx).is_some() {
        return;
    }
    let progress = Arc::new(Progress::default());
    cx.set_global(Running(Some(progress.clone())));
    // Keeps the menubar chip and the tasks window ticking; nothing
    // observes an app-global pass on its own.
    crate::tasks_window::repaint_while_running(cx);
    // Quitting mid-pass raises the same flag the stop button does, so the
    // worker stops between values rather than being killed mid-write.
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
            let db_path = library.db_path();
            // The same stale set the prompt counted with, so the run reads
            // exactly the rows it promised, older spellings included.
            let stale = stale(&db_path);
            let work = library
                .projection()
                .map(|projection| backlog(projection, &stale).items)
                .unwrap_or_default();
            (db_path, work)
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
            // What the run cost, so the next prompt opens with a number
            // instead of an offer to go and find one.
            if let Some(pace) = progress.secs_per_value() {
                rox_core::settings::Settings::update(move |s| s.session.romanize_pace = pace);
            }
            cx.set_global(Running(None));
            match written {
                Ok(0) => {}
                Ok(written) => {
                    log::info!("romanize: {written} values filled");
                    // The rows went into rox's own tables, which the
                    // projection lays over the symbol tables and the title
                    // arenas as it builds, so a reload is what actually
                    // moves the letter rails and teaches search the Latin
                    // spellings.
                    library.update(cx, |library, cx| library.reload_projection(cx));
                }
                Err(e) => {
                    log::error!("romanize: {e}");
                }
            }
        })
        .ok();
    })
    .detach();
}

/// The blocking half: one value at a time, read it, store what came back.
/// Returns how many values took a sort name.
///
/// One dictionary load for the whole run, and a transaction per [`BATCH`]
/// values. A value that reads as nothing stores nothing, so it's looked at
/// again on the next run, which is the right answer for a library that
/// installs the dictionary afterwards.
fn run(db_path: &Path, work: Vec<Item>, progress: &Progress) -> Result<usize, String> {
    let mut conn = store::open(db_path).map_err(|e| e.to_string())?;
    progress.total.store(work.len(), Ordering::Relaxed);
    progress.pace.begin();
    let ja = load_dictionary(&work);
    // Every row this run writes carries this build's spelling version, so
    // the next build that reads differently can find them.
    let marker = artist_meta::romanized_marker(rox_romanize::VERSION);

    let mut written = 0usize;
    for batch in work.chunks(BATCH) {
        if !progress.keep_going() {
            break;
        }
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        for item in batch {
            if !progress.keep_going() {
                break;
            }
            *progress.current.lock().unwrap() = item.value.clone();
            match rox_romanize::romanize_as(&item.value, ja, item.reading) {
                Some(sort) => {
                    match &item.target {
                        Target::Title(rows) => {
                            for &id in rows {
                                track_meta::set(&tx, id, &sort, &marker)
                                    .map_err(|e| e.to_string())?;
                            }
                        }
                        Target::Album => {
                            album_meta::set(&tx, &item.value, &sort, &marker)
                                .map_err(|e| e.to_string())?;
                        }
                        Target::Artist => {
                            artist_meta::set(&tx, &item.value, &sort, &marker)
                                .map_err(|e| e.to_string())?;
                        }
                    }
                    written += 1;
                }
                None => {
                    progress.failed.fetch_add(1, Ordering::Relaxed);
                    // Told apart from a script nothing here reads, because
                    // this one is a download away from working.
                    if ja.is_none() && rox_romanize::needs_dictionary(&item.value, item.reading) {
                        progress.skipped.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            progress.done.fetch_add(1, Ordering::Relaxed);
        }
        tx.commit().map_err(|e| e.to_string())?;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table built by hand rather than through a projection, the shape
    /// [`crate::sortnames_job`]'s tests use: the scope is a question about
    /// symbols and their sort names, and a scratch database would only be
    /// a slower way to write these rows.
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

    fn values(table: &SymTable, japanese: &HashSet<u32>) -> Vec<Item> {
        let mut out = Vec::new();
        let mut considered = 0;
        collect(
            table,
            japanese,
            Target::Artist,
            &HashSet::new(),
            &mut HashSet::new(),
            &mut out,
            &mut considered,
        );
        out
    }

    /// A value an older spelling filled is read again; one this spelling
    /// filled, or a person or service filled, is not.
    #[test]
    fn a_stale_reading_is_back_in_scope() {
        let table = table(&[("秋ノ風", "akinokaze"), ("米津玄師", "Yonezu, Kenshi")]);
        let mut out = Vec::new();
        let mut considered = 0;
        let stale: HashSet<String> = HashSet::from(["秋ノ風".to_string()]);
        collect(
            &table,
            &HashSet::new(),
            Target::Artist,
            &stale,
            &mut HashSet::new(),
            &mut out,
            &mut considered,
        );
        let names: Vec<&str> = out.iter().map(|item| item.value.as_str()).collect();
        assert_eq!(names, ["秋ノ風"]);
        assert_eq!(considered, 2);
    }

    #[test]
    fn only_values_with_something_to_read_and_nowhere_to_file_are_in_scope() {
        let table = table(&[
            ("米津玄師", ""),
            ("서태지", ""),
            // Latin, accents and all: it folds to ASCII, so it already
            // files where a person would look for it.
            ("Beyoncé", ""),
            ("Zebra", ""),
            // Already answered, by MusicBrainz or by an earlier run.
            ("崎山蒼志", "Sakiyama, Soushi"),
            ("", ""),
        ]);
        let items = values(&table, &HashSet::new());
        let names: Vec<&str> = items.iter().map(|item| item.value.as_str()).collect();
        assert_eq!(names, ["米津玄師", "서태지"]);
    }

    #[test]
    fn a_value_seen_beside_kana_is_read_as_japanese() {
        let table = table(&[("東京", ""), ("北京", "")]);
        // The first symbol turned up on a row with kana on it, the second
        // didn't.
        let japanese = HashSet::from([0u32]);
        let items = values(&table, &japanese);
        assert_eq!(items[0].reading, Reading::Japanese);
        assert_eq!(items[1].reading, Reading::Auto);
        // Which is what decides how many values get skipped without the
        // download.
        let backlog = Backlog {
            items: items.clone(),
            considered: 2,
        };
        assert_eq!(backlog.kanji(), 1);
        let chinese = Backlog {
            items: items[1..].to_vec(),
            considered: 1,
        };
        assert_eq!(chinese.kanji(), 0);
    }

    #[test]
    fn a_value_in_both_artist_tables_is_read_once() {
        let artists = table(&[("米津玄師", ""), ("崎山蒼志", "")]);
        let album_artists = table(&[("米津玄師", ""), ("서태지", "")]);
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        let mut considered = 0;
        for t in [&artists, &album_artists] {
            collect(
                t,
                &HashSet::new(),
                Target::Artist,
                &HashSet::new(),
                &mut seen,
                &mut out,
                &mut considered,
            );
        }
        let names: Vec<&str> = out.iter().map(|item| item.value.as_str()).collect();
        assert_eq!(names, ["米津玄師", "崎山蒼志", "서태지"]);
        // Every distinct value was counted, including the ones with
        // nothing to read.
        assert_eq!(considered, 3);
    }

    /// A row for the projection tests, everything but the four fields the
    /// backlog reads left at whatever parses.
    fn row(
        path: &str,
        title: &str,
        title_sort: &str,
        artist: &str,
        album_artist: &str,
        album: &str,
    ) -> rox_library::TrackRow {
        rox_library::TrackRow {
            path: path.to_string(),
            sub: 0,
            cue: None,
            title: title.to_string(),
            artist: artist.to_string(),
            album_artist: album_artist.to_string(),
            album: album.to_string(),
            title_sort: title_sort.to_string(),
            artist_sort: String::new(),
            album_artist_sort: String::new(),
            album_sort: String::new(),
            genre: String::new(),
            year: 2000,
            disc_no: 1,
            track_no: 1,
            duration_ms: 1000,
            codec: "flac".into(),
            bitrate_kbps: 900,
            sample_rate_hz: 44100,
            bit_depth: 16,
            rating: 0,
            replay_gain: Default::default(),
            bpm: None,
            size: 0,
            mtime: 0,
        }
    }

    /// A projection over an in-memory database, the same load path the
    /// catalog runs.
    fn projection(rows: &[rox_library::TrackRow]) -> Projection {
        let mut conn = rox_library::rusqlite::Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();
        store::insert_batch(&mut conn, rows).unwrap();
        Projection::load_serial(&conn, false).unwrap()
    }

    fn reading_of(backlog: &Backlog, value: &str) -> Option<Reading> {
        backlog
            .items
            .iter()
            .find(|item| item.value == value)
            .map(|item| item.reading)
    }

    /// The row that proves a title is Japanese doesn't have to be a row
    /// this pass touches. Here the kana row already has a sort title, so
    /// the pass skips it, and the language it settles still has to reach
    /// the duplicate title that doesn't.
    #[test]
    fn a_settled_row_still_says_what_language_its_title_is_in() {
        let projection = projection(&[
            row("/a.flac", "東京", "Tokyo", "Zebra", "Zebra", "レモン"),
            row("/b.flac", "東京", "", "Zebra", "Zebra", "Album"),
        ]);
        let backlog = backlog(&projection, &Stale::default());
        assert_eq!(reading_of(&backlog, "東京"), Some(Reading::Japanese));
    }

    /// The three symbol tables number their symbols apart, so what's
    /// Japanese in one of them says nothing about the same number in
    /// another. Pooled together, the artist on the third row here inherits
    /// an album artist's id and gets read as Japanese.
    #[test]
    fn each_symbol_table_keeps_its_own_idea_of_japanese() {
        let projection = projection(&[
            row("/a.flac", "T1", "", "ヒカル", "ヒカル", "Album"),
            row("/b.flac", "T2", "", "ヒカル", "Beta", "Album"),
            row("/c.flac", "T3", "", "東京", "Beta", "Other"),
        ]);
        let backlog = backlog(&projection, &Stale::default());
        assert_eq!(reading_of(&backlog, "ヒカル"), Some(Reading::Japanese));
        assert_eq!(reading_of(&backlog, "東京"), Some(Reading::Auto));
    }

    /// Only the kanji values get skipped without the download. Everything
    /// else in the backlog runs on a fresh install, which is why the pass
    /// starts either way and reports what it left.
    #[test]
    fn only_kanji_values_are_skipped_without_the_download() {
        let mut items = values(&table(&[("서태지", ""), ("레몬", "")]), &HashSet::new());
        items.extend(values(&table(&[("レモン", "")]), &HashSet::new()));
        let none = Backlog {
            considered: items.len() as u64,
            items,
        };
        assert_eq!(none.kanji(), 0, "hangul and kana are tables, not lookups");

        let mut items = values(&table(&[("君の名は", ""), ("서태지", "")]), &HashSet::new());
        items.extend(values(&table(&[("レモン", "")]), &HashSet::new()));
        let mixed = Backlog {
            considered: items.len() as u64,
            items,
        };
        // One of the three, and the other two still have somewhere to go.
        assert_eq!(mixed.kanji(), 1);
        assert_eq!(mixed.items.len(), 3);
    }
}
