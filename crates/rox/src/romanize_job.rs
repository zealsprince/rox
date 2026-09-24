//! The romanization pass: read every non-Latin title, album and artist the
//! library has no sort name for, and store what it says in Latin letters.
//! [`crate::sortnames_job`] fills artists from MusicBrainz; nothing has sort
//! names for titles or albums, so this reads the characters through
//! [`rox_romanize`]. Nothing here writes a file or talks to a service.
//!
//! Deduplicated by value, applied by row: titles have no symbol in the
//! projection, so their answers are written per track id.
//!
//! A Han-only value that appears anywhere alongside kana is read as Japanese,
//! everything else as Mandarin. That's the one thing [`rox_romanize`] can't
//! work out itself, and why it takes a [`Reading`].
//!
//! Kanji needs the IPADIC download ([`rox_romanize::dictionary`]). Without it
//! those values are skipped and counted, and the rest still runs.

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

/// Values per transaction: one per row would fsync a thousand times, one over
/// the whole backlog would hold the write lock for the pass and lose everything
/// to a stop.
const BATCH: usize = 1_000;

const SAMPLE: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Every track carrying this exact title and no sort title of its own;
    /// titles aren't interned.
    Title(Vec<i64>),
    Album,
    /// One [`artist_meta`] row serves both artist tables.
    Artist,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub value: String,
    pub reading: Reading,
    pub target: Target,
}

#[derive(Default)]
pub struct Backlog {
    pub items: Vec<Item>,
    pub considered: u64,
}

impl Backlog {
    pub fn kanji(&self) -> u64 {
        self.items
            .iter()
            .filter(|item| rox_romanize::needs_dictionary(&item.value, item.reading))
            .count() as u64
    }
}

/// The fold test [`crate::sortnames_job::is_latin`] uses: Latin script folds to
/// ASCII.
fn worth_reading(value: &str) -> bool {
    !value.is_empty() && !rox_library::fold::fold(value).is_ascii()
}

/// Rows an older spelling of this pass wrote, read again: the projection shows
/// them as filled.
#[derive(Default, Clone, Debug)]
pub struct Stale {
    pub titles: HashSet<i64>,
    pub albums: HashSet<String>,
    pub artists: HashSet<String>,
}

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

/// Also works out which values appear beside kana, which is how a bare-kanji
/// value gets its language. A value in `stale` counts as unfilled.
pub fn backlog(projection: &Projection, stale: &Stale) -> Backlog {
    // One set per interner: the tables number symbols independently, so pooling
    // them marks the wrong names Japanese.
    let mut japanese_artists: HashSet<u32> = HashSet::new();
    let mut japanese_album_artists: HashSet<u32> = HashSet::new();
    let mut japanese_albums: HashSet<u32> = HashSet::new();
    // By value, settled after the walk: the row that proves a title Japanese
    // may be one this pass skips, or come later.
    let mut japanese_titles: HashSet<&str> = HashSet::new();
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
        // Kana anywhere on the row settles the language for everything on it.
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
        // A sort title from an older spelling doesn't count as settled.
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
    // Both artist tables feed one list: the answer is for the value, not the
    // column.
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
        // Marked before the sort-name check, so the prompt's count matches the
        // list the pass works through.
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

#[derive(Clone, Copy, Default)]
pub struct Coverage {
    pub missing: u64,
    pub total: u64,
    pub kanji: u64,
}

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

pub fn dictionary_installed() -> bool {
    rox_romanize::dictionary::IPADIC.installed()
}

/// A measured pace rather than a constant: kana is a table lookup per
/// character, kanji a Viterbi lattice per title.
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

/// None when nothing needs it or the download is missing.
/// [`rox_romanize::japanese`] keeps one mapping per process, shared with the
/// metadata panel's Romanize button.
fn load_dictionary(items: &[Item]) -> Option<&'static Japanese> {
    items
        .iter()
        .any(|item| rox_romanize::needs_dictionary(&item.value, item.reading))
        .then(rox_romanize::japanese)
        .flatten()
}

#[derive(Default)]
pub struct Progress {
    done: AtomicUsize,
    total: AtomicUsize,
    /// Nothing is stored for these, so they come back on the next run.
    failed: AtomicUsize,
    /// Kanji with no dictionary, a subset of `failed` counted apart because
    /// installing the download fixes it.
    skipped: AtomicUsize,
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

    pub fn skipped(&self) -> usize {
        self.skipped.load(Ordering::Relaxed)
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

    pub fn secs_per_value(&self) -> Option<f32> {
        self.pace
            .secs_per_track(self.done())
            .map(|secs| secs as f32)
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

/// A no-op while a pass is already running. Safe inside the library's own
/// update: the work list is read in the spawned task.
pub fn start(library: Entity<Library>, cx: &mut App) {
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
            let db_path = library.db_path();
            // The same stale set the prompt counted with.
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
            if let Some(pace) = progress.secs_per_value() {
                rox_core::settings::Settings::update(move |s| s.session.romanize_pace = pace);
            }
            cx.set_global(Running(None));
            match written {
                Ok(0) => {}
                Ok(written) => {
                    log::info!("romanize: {written} values filled");
                    // A reload is what moves the letter rails and teaches
                    // search the Latin spellings.
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

/// A value that reads as nothing stores nothing, so the next run looks again.
fn run(db_path: &Path, work: Vec<Item>, progress: &Progress) -> Result<usize, String> {
    let mut conn = store::open(db_path).map_err(|e| e.to_string())?;
    progress.total.store(work.len(), Ordering::Relaxed);
    progress.pace.begin();
    let ja = load_dictionary(&work);
    // Stamped with this build's spelling version, so a later build can find and
    // redo them.
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
                    // Counted apart: installing the dictionary fixes these.
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
            ("Beyoncé", ""),
            ("Zebra", ""),
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
        let japanese = HashSet::from([0u32]);
        let items = values(&table, &japanese);
        assert_eq!(items[0].reading, Reading::Japanese);
        assert_eq!(items[1].reading, Reading::Auto);
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
        assert_eq!(considered, 3);
    }

    fn row(
        path: &str,
        title: &str,
        title_sort: &str,
        artist: &str,
        album_artist: &str,
        album: &str,
    ) -> rox_library::TrackRow {
        rox_library::TrackRow {
            remote_url: String::new(),
            remote_live: false,
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

    /// The kana row already has a sort title, so the pass skips it, but its
    /// language still has to reach the duplicate title.
    #[test]
    fn a_settled_row_still_says_what_language_its_title_is_in() {
        let projection = projection(&[
            row("/a.flac", "東京", "Tokyo", "Zebra", "Zebra", "レモン"),
            row("/b.flac", "東京", "", "Zebra", "Zebra", "Album"),
        ]);
        let backlog = backlog(&projection, &Stale::default());
        assert_eq!(reading_of(&backlog, "東京"), Some(Reading::Japanese));
    }

    /// The artist on the third row shares an album artist's symbol number and
    /// must not be read as Japanese.
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
        assert_eq!(mixed.kanji(), 1);
        assert_eq!(mixed.items.len(), 3);
    }
}
