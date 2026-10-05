//! Non-local sources and the sync that turns one into library rows, starting
//! with Subsonic. A sync is a reconcile: every song upserts under
//! `subsonic:<digest>` and anything that id no longer lists is pruned. Both
//! halves are scoped to the source string, so a sync can never reach a local
//! row. Each server digests to its own id, so catalogs never share a row.
//!
//! This module owns every `subsonic:` id, so a row under one no configured
//! account digests to came from an address an account left, and it goes:
//! the authorizer won't sign for it, so it could never play. It answers for
//! which `plugin:` ids are live and kept too (ADR 30), from the plugin
//! records.
//!
//! A remote row stores only its stream URL; credentials never go in SQLite.
//! The authorize table finishes each request from settings (for Subsonic, a
//! fresh salt and token on the URL).

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{App, Entity, Task};

use rox_core::settings::{AccountsState, PluginRecord, Settings, SubsonicAccount};
use rox_library::TrackRow;
use rox_library::cue::{Origin, PLUGIN_PREFIX};
use rox_library::members;
use rox_library::playlists;
use rox_library::replaygain::ReplayGain;
use rox_library::rusqlite::Connection;
use rox_library::stations::{self, Station};
use rox_library::store;
use rox_net::sources::subsonic::{Server, ServerInfo};
use rox_net::sources::{SourceStation, SourceTrack};

use crate::catalog::Library;
use crate::sources_registry;

/// The thumbnail store downscales again; this only has to beat a grid tile.
const COVER_SIZE: u32 = 512;

/// The server scales down to this, so a full-resolution scan never comes
/// over the wire.
const FULL_COVER_SIZE: u32 = 1280;

/// Long enough that a down server isn't hammered by every repaint.
const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

const MISSES_SWEEP: usize = 4096;

/// Subsonic's namespace. A test holds it to [`Server::source_id`].
const SOURCE_PREFIX: &str = "subsonic:";

struct Progress {
    running: AtomicBool,
    done: AtomicUsize,
    total: AtomicUsize,
}

static PROGRESS: Progress = Progress {
    running: AtomicBool::new(false),
    done: AtomicUsize::new(0),
    total: AtomicUsize::new(0),
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    pub tracks: usize,
    pub pruned: usize,
    /// Rows of an address the account has left, apart from `pruned`.
    pub departed: usize,
    pub playlists: usize,
    pub stations: usize,
}

/// The source id the running sync is walking, so the settings page puts the
/// album count on the right line.
static SYNCING: Mutex<Option<String>> = Mutex::new(None);

pub fn progress() -> Option<(usize, usize)> {
    if !PROGRESS.running.load(Ordering::Relaxed) {
        return None;
    }

    Some((
        PROGRESS.done.load(Ordering::Relaxed),
        PROGRESS.total.load(Ordering::Relaxed),
    ))
}

/// One sync at a time across every server: two would race each other's prune.
pub fn syncing() -> bool {
    PROGRESS.running.load(Ordering::Relaxed)
}

pub fn syncing_source() -> Option<String> {
    SYNCING.lock().ok()?.clone()
}

fn accounts() -> Vec<SubsonicAccount> {
    Settings::load().accounts.subsonic_servers
}

/// For what reads Subsonic and plugins together.
fn accounts_state() -> AccountsState {
    Settings::load().accounts
}

/// The source a plugin's rows file under.
pub fn plugin_source(record: &PluginRecord) -> String {
    format!("{PLUGIN_PREFIX}{}", record.id)
}

/// None while it names no address.
fn server_of(account: &SubsonicAccount) -> Option<Server> {
    if account.url.trim().is_empty() {
        return None;
    }

    Some(Server::new(&account.url, &account.user, &account.password))
}

pub fn source_of(account: &SubsonicAccount) -> Option<String> {
    server_of(account).map(|server| server.source_id())
}

/// Answers for a switched-off account too: Connect has to work first.
pub fn server(index: usize) -> Option<Server> {
    server_of(accounts().get(index)?)
}

/// Everything that reaches a server on the library's behalf goes through
/// here rather than [`server`], so a switched-off server is never used.
fn live_servers() -> Vec<Server> {
    accounts()
        .iter()
        .filter(|account| account.enabled)
        .filter_map(server_of)
        .collect()
}

fn live_server(source: &str) -> Option<Server> {
    live_servers()
        .into_iter()
        .find(|server| server.source_id() == source)
}

/// Fill the library's source-name table (see
/// [`rox_library::cue::source_label`]). Run on every projection load.
pub fn publish_labels() {
    rox_library::cue::set_source_labels(labels(&accounts_state()));
}

fn labels(accounts: &AccountsState) -> HashMap<String, String> {
    let mut labels = HashMap::new();
    labels.insert(
        rox_library::cue::LOCAL.to_string(),
        rox_i18n::t!("metadata-source-local").to_string(),
    );
    labels.insert(
        stations::SOURCE.to_string(),
        rox_i18n::t!("metadata-source-radio").to_string(),
    );
    labels.insert(
        rox_library::unknown::SOURCE.to_string(),
        rox_i18n::t!("metadata-source-unknown").to_string(),
    );

    for account in &accounts.subsonic_servers {
        let Some(source) = source_of(account) else {
            continue;
        };

        let label = account.label();
        if !label.is_empty() {
            labels.insert(source, label);
        }
    }

    // A plugin with no record keeps the raw id, which is how a removed
    // plugin's leftover rows read.
    for record in &accounts.plugins {
        if !record.label.is_empty() {
            labels.insert(plugin_source(record), record.label.clone());
        }
    }

    labels
}

/// Called at startup, before anything can resolve a row, and again after
/// the settings change.
pub fn install_registry() {
    for server in accounts().iter().filter_map(server_of) {
        let source = server.source_id();
        let mine = source.clone();

        sources_registry::install(
            &source,
            // Rebuilt from settings on each call, so a changed password needs
            // no reinstall. Only a switched-on account with this very id signs.
            Box::new(move |remote| {
                let Some(server) = live_server(&mine) else {
                    return;
                };

                remote.headers = server.stream_headers();
                // The salt is fresh per request, so the token only ever goes
                // on here, never into the stored URL.
                remote.url = server.sign(&remote.url);
            }),
        );
    }
}

/// Switched-on Subsonic accounts in list order, then switched-on plugins
/// whose folder `present` finds. A Missing plugin's rows hide like a
/// switched-off one's.
fn live_in_order(accounts: &AccountsState, present: &dyn Fn(&str) -> bool) -> Vec<String> {
    let servers = accounts
        .subsonic_servers
        .iter()
        .filter(|account| account.enabled)
        .filter_map(source_of);

    let plugins = accounts
        .plugins
        .iter()
        .filter(|record| record.enabled && present(&record.id))
        .map(plugin_source);

    servers.chain(plugins).collect()
}

/// The non-local ids whose rows browse, against the last scan of the
/// plugins folder.
pub fn live_ids(accounts: &AccountsState) -> HashSet<String> {
    live_ids_in(accounts, &crate::plugins::present)
}

fn live_ids_in(accounts: &AccountsState, present: &dyn Fn(&str) -> bool) -> HashSet<String> {
    live_in_order(accounts, present).into_iter().collect()
}

/// What an everyday prune keeps, one set per namespace. None leaves that
/// namespace alone.
struct Kept {
    subsonic: Option<HashSet<String>>,
    plugins: Option<HashSet<String>>,
}

impl Kept {
    fn departs(&self, source: &str) -> bool {
        let kept = if source.starts_with(SOURCE_PREFIX) {
            &self.subsonic
        } else if source.starts_with(PLUGIN_PREFIX) {
            &self.plugins
        } else {
            return false;
        };

        kept.as_ref().is_some_and(|kept| !kept.contains(source))
    }
}

/// Every configured id, switched on or not: a switched-off account or plugin
/// keeps its rows.
///
/// An empty address is a half-finished edit, and an empty list is more likely
/// a file that didn't load than an instruction, so either leaves its namespace
/// unpruned ([`remove`] handles its own rows).
fn kept_ids(accounts: &AccountsState) -> Kept {
    let servers = &accounts.subsonic_servers;
    let subsonic = match servers.is_empty() {
        true => None,
        false => servers.iter().map(source_of).collect(),
    };

    Kept {
        subsonic,
        plugins: kept_plugins(&accounts.plugins),
    }
}

fn kept_plugins(records: &[PluginRecord]) -> Option<HashSet<String>> {
    if records.is_empty() {
        return None;
    }

    Some(records.iter().map(plugin_source).collect())
}

/// Referrers (playlist entries, listens, thumbnails) outlive the rows, the
/// same as they outlive a track the server dropped.
fn drop_departed(conn: &mut Connection, kept: &Kept) -> usize {
    let departed: Vec<String> = store::sources(conn)
        .unwrap_or_default()
        .into_iter()
        .map(|(source, _)| source)
        .filter(|source| kept.departs(source))
        .collect();

    let nothing = HashSet::new();
    departed
        .iter()
        .filter_map(|source| match source.starts_with(PLUGIN_PREFIX) {
            // Membership goes with the rows, or a leftover would hold a
            // collection for a plugin nobody has.
            true => members::remove_source(conn, source).ok(),
            false => store::prune_source(conn, source, &nothing).ok(),
        })
        .sum()
}

fn prune_for(conn: &mut Connection, accounts: &AccountsState) -> usize {
    drop_departed(conn, &kept_ids(accounts))
}

/// The departure pass, run once per launch on the library's own connection
/// before its first projection. `accounts.json` can be edited by hand while
/// rox is closed, and without this a library with no Subsonic server never
/// prunes: a removed plugin's rows would sit hidden forever.
pub fn depart_at_startup(conn: &mut Connection) -> usize {
    static DONE: AtomicBool = AtomicBool::new(false);

    depart_once(conn, &accounts_state(), &DONE)
}

fn depart_once(conn: &mut Connection, accounts: &AccountsState, done: &AtomicBool) -> usize {
    // A sync prunes on its own, and two writers on one database is a busy error.
    if syncing() || done.swap(true, Ordering::Relaxed) {
        return 0;
    }

    let gone = prune_for(conn, accounts);
    if gone > 0 {
        log::info!("sources: {gone} rows of departed sources removed at startup");
    }

    gone
}

/// What the settings page calls after an address, login, or switch changes:
/// the old address's rows go now, and the catalog reloads, since the
/// switches decide which rows browse ([`hidden_sources`]).
pub fn follow_accounts(library: Entity<Library>, cx: &mut App) -> Task<usize> {
    // A sync does this itself at the end, and two writers on one database is
    // a busy error.
    if syncing() {
        return Task::ready(0);
    }

    let accounts = accounts_state();
    let db_path = library.read(cx).db_path();

    cx.spawn(async move |cx| {
        let gone = cx
            .background_executor()
            .spawn(async move {
                match store::open(&db_path) {
                    Ok(mut conn) => prune_for(&mut conn, &accounts),
                    Err(e) => {
                        log::warn!("subsonic: pruning departed rows failed: {e}");
                        0
                    }
                }
            })
            .await;

        library
            .update(cx, |library, cx| library.reload_projection(cx))
            .ok();

        gone
    })
}

/// What Remove Server runs. Also drops rows of any address the account had
/// since left, when the remaining accounts can all be trusted.
pub fn remove(index: usize, library: Entity<Library>, cx: &mut App) -> Task<usize> {
    // A running sync's rows would land after the delete.
    if syncing() {
        return Task::ready(0);
    }

    let mut accounts = accounts_state();
    if index >= accounts.subsonic_servers.len() {
        return Task::ready(0);
    }

    let removed = accounts.subsonic_servers.remove(index);
    let gone_source = source_of(&removed);

    if let Some(source) = &gone_source {
        sources_registry::forget(source);
    }

    Settings::update(move |s| {
        if index < s.accounts.subsonic_servers.len() {
            s.accounts.subsonic_servers.remove(index);
        }
    });

    let db_path = library.read(cx).db_path();

    cx.spawn(async move |cx| {
        let gone = cx
            .background_executor()
            .spawn(async move {
                let mut conn = match store::open(&db_path) {
                    Ok(conn) => conn,
                    Err(e) => {
                        log::warn!("subsonic: removing a server's rows failed: {e}");
                        return 0;
                    }
                };

                remove_rows(&mut conn, gone_source.as_deref(), &accounts)
            })
            .await;

        library
            .update(cx, |library, cx| library.reload_projection(cx))
            .ok();

        gone
    })
}

/// Unlike the everyday prune, an empty server list is trusted here: somebody
/// just asked for exactly that. Plugins keep the everyday rule, since nobody
/// asked about them.
fn remove_rows(conn: &mut Connection, source: Option<&str>, left: &AccountsState) -> usize {
    let nothing = HashSet::new();
    let own = source
        .and_then(|source| store::prune_source(conn, source, &nothing).ok())
        .unwrap_or(0);

    let kept = Kept {
        subsonic: left.subsonic_servers.iter().map(source_of).collect(),
        plugins: kept_plugins(&left.plugins),
    };
    let departed = drop_departed(conn, &kept);

    own + departed
}

/// Every Subsonic or plugin id that isn't switched on, as the projection load
/// asks it. Reads the settings once.
pub fn hidden_sources() -> impl Fn(&str) -> bool {
    let live = live_ids(&accounts_state());

    move |source| hides(&live, source)
}

fn hides(live: &HashSet<String>, source: &str) -> bool {
    let switchable = source.starts_with(SOURCE_PREFIX) || source.starts_with(PLUGIN_PREFIX);

    switchable && !live.contains(source)
}

pub fn ping(index: usize, cx: &App) -> Task<Result<ServerInfo, String>> {
    let server = server(index);

    cx.background_executor().spawn(async move {
        let server = server.ok_or_else(|| "no server configured".to_string())?;

        server.ping()
    })
}

/// On its own connection on the background executor; the projection
/// reloads once at the end.
pub fn sync(
    index: usize,
    library: Entity<Library>,
    cx: &mut App,
) -> Task<Result<SyncOutcome, String>> {
    let Some(server) = server(index) else {
        return Task::ready(Err("no server configured".to_string()));
    };

    if PROGRESS.running.swap(true, Ordering::SeqCst) {
        return Task::ready(Err("a sync is already running".to_string()));
    }

    PROGRESS.done.store(0, Ordering::Relaxed);
    PROGRESS.total.store(0, Ordering::Relaxed);

    let source = server.source_id();
    if let Ok(mut syncing) = SYNCING.lock() {
        *syncing = Some(source.clone());
    }

    // Install the registry first: a re-pointed account's new rows would
    // otherwise have no authorizer until the next launch.
    install_registry();

    let db_path = library.read(cx).db_path();
    let accounts = accounts_state();

    cx.spawn(async move |cx| {
        let outcome = cx
            .background_executor()
            .spawn(async move {
                let mut conn = store::open(&db_path).map_err(|e| e.to_string())?;

                run(&server, &accounts, &mut conn)
            })
            .await;

        PROGRESS.running.store(false, Ordering::Relaxed);
        if let Ok(mut syncing) = SYNCING.lock() {
            *syncing = None;
        }

        if outcome.is_ok() {
            // Stamped by source id: the list can change under a long sync.
            let now = now_secs();
            Settings::update(move |s| {
                if let Some(account) = s
                    .accounts
                    .subsonic_servers
                    .iter_mut()
                    .find(|account| source_of(account).as_deref() == Some(source.as_str()))
                {
                    account.last_sync = now;
                }
            });

            library
                .update(cx, |library, cx| library.reload_projection(cx))
                .ok();
        }

        outcome
    })
}

/// `accounts` is the list as the sync started, which decides which other
/// servers' rows are still somebody's.
fn run(
    server: &Server,
    accounts: &AccountsState,
    conn: &mut Connection,
) -> Result<SyncOutcome, String> {
    let source = server.source_id();

    let tracks = server.catalog(|done, total| {
        PROGRESS.done.store(done, Ordering::Relaxed);
        PROGRESS.total.store(total, Ordering::Relaxed);
    })?;

    let now = now_secs();
    let rows: Vec<TrackRow> = tracks.iter().map(|track| row_for(track, now)).collect();

    let keep: HashSet<String> = tracks.iter().map(|track| track.id.clone()).collect();
    let (pruned, departed) = reconcile(conn, &source, &rows, &keep, accounts)?;

    let playlists = sync_playlists(server, conn, &source, now);
    let stations = sync_stations(server, conn);

    Ok(SyncOutcome {
        tracks: rows.len(),
        pruned,
        departed,
        playlists,
        stations,
    })
}

/// The departed pass runs here too, since `accounts.json` can be edited by
/// hand without the settings window ever opening.
fn reconcile(
    conn: &mut Connection,
    source: &str,
    rows: &[TrackRow],
    keep: &HashSet<String>,
    accounts: &AccountsState,
) -> Result<(usize, usize), String> {
    store::upsert_source_rows(conn, source, rows).map_err(|e| e.to_string())?;

    let pruned = store::prune_source(conn, source, keep).map_err(|e| e.to_string())?;
    let departed = prune_for(conn, accounts);

    Ok((pruned, departed))
}

/// A playlist that already exists by name is left alone: rox can't tell a
/// stale import from a local edit. Playlist trouble never fails the sync.
fn sync_playlists(server: &Server, conn: &mut Connection, source: &str, now: i64) -> usize {
    let Ok(remote) = server.playlists() else {
        return 0;
    };

    let existing: HashSet<String> = playlists::list(conn)
        .unwrap_or_default()
        .into_iter()
        .map(|playlist| playlist.name)
        .collect();

    let mut created = 0;
    for list in remote {
        if list.name.is_empty() || existing.contains(&list.name) {
            continue;
        }

        // A song the catalog didn't return is skipped, not fatal to the list.
        let track_ids: Vec<i64> = list
            .track_ids
            .iter()
            .filter_map(|id| store::id_for_path(conn, source, id).ok().flatten())
            .collect();

        if track_ids.is_empty() {
            continue;
        }

        let Ok(playlist_id) = playlists::create(conn, &list.name, now) else {
            continue;
        };

        if playlists::add(conn, playlist_id, &track_ids, now).is_ok() {
            created += 1;
        }
    }

    created
}

/// Keyed on the stream URL, so a re-sync updates a renamed station in place.
/// A station the server removed isn't dropped: the radio list doesn't
/// remember where a station came from.
fn sync_stations(server: &Server, conn: &mut Connection) -> usize {
    let Ok(remote) = server.radio_stations() else {
        return 0;
    };

    let found: Vec<Station> = remote.iter().map(station_for).collect();
    if found.is_empty() {
        return 0;
    }

    match stations::put(conn, &found) {
        Ok(()) => found.len(),
        Err(e) => {
            log::warn!("subsonic: writing {} stations failed: {e}", found.len());
            0
        }
    }
}

/// The URL is a station's identity, so the same stream typed by hand and
/// listed by the server is one row.
fn station_for(station: &SourceStation) -> Station {
    Station {
        url: station.stream_url.clone(),
        name: station.name.clone(),
        genre: String::new(),
    }
}

/// Every surface draws a remote row's art through here, so they all agree.
/// Blocking.
pub fn art(thumbs: &Mutex<Connection>, key: &str) -> Option<Vec<u8>> {
    rox_library::thumbs::thumbnail(thumbs, Path::new(key)).or_else(|| cover(thumbs, key))
}

/// Fetched on demand, never during the sync. Only a key a switched-on server
/// has a row under reaches a server: never hand one a local path as a song
/// id. An empty answer waits out [`RETRY_AFTER`], or a down server costs a
/// timeout per visible row per catalog change. Blocking; the store lock is
/// only taken for the write.
pub fn cover(thumbs: &Mutex<Connection>, key: &str) -> Option<Vec<u8>> {
    // A key that stats is a file; the store already answered for it.
    if std::fs::metadata(key).is_ok() {
        return None;
    }

    let now = Instant::now();
    if MISSES.lock().ok()?.recent(key, now) {
        return None;
    }

    let found = fetch_cover(thumbs, key);

    if let Ok(mut misses) = MISSES.lock() {
        match found {
            Some(_) => misses.forget(key),
            None => misses.note(key, now),
        }
    }

    found
}

/// The first live source holding the key answers. The library has no column
/// for a Subsonic art id, so the song is asked for it once; after that the
/// store answers.
fn fetch_cover(thumbs: &Mutex<Connection>, key: &str) -> Option<Vec<u8>> {
    let accounts = accounts_state();
    let live = live_in_order(&accounts, &crate::plugins::present);
    if live.is_empty() {
        return None;
    }

    let library = store::open(&rox_core::settings::data_dir().join("library.db")).ok()?;
    let source = live
        .into_iter()
        .find(|source| matches!(store::id_for_path(&library, source, key), Ok(Some(_))))?;

    match Origin::of(&source) {
        Origin::Subsonic => {
            let bytes = subsonic_cover(&accounts, &source, key, COVER_SIZE)?;
            let thumbs = thumbs.lock().ok()?;

            rox_library::thumbs::store_bytes(&thumbs, &bytes, key)
        }

        Origin::Plugin => plugin_thumb(thumbs, &source, key),

        Origin::Local | Origin::Radio | Origin::Unknown => None,
    }
}

fn subsonic_cover(accounts: &AccountsState, source: &str, key: &str, size: u32) -> Option<Vec<u8>> {
    let server = accounts
        .subsonic_servers
        .iter()
        .filter_map(server_of)
        .find(|server| server.source_id() == source)?;

    let cover_id = server.cover_id(key).ok()?;
    if cover_id.is_empty() {
        return None;
    }

    server.cover(&cover_id, size).ok()
}

/// A remote track's cover at the size a panel draws it. The thumbnail store
/// only keeps list-row cuts, which go soft stretched across a panel, so
/// nothing here is stored. Blocking.
pub fn full_cover(source: &str, key: &str) -> Option<Vec<u8>> {
    let accounts = accounts_state();
    let live = live_in_order(&accounts, &crate::plugins::present);
    if !live.iter().any(|id| id == source) {
        return None;
    }

    let miss = format!("{}|full", plugin_thumb_key(source, key));
    let now = Instant::now();
    if MISSES.lock().ok()?.recent(&miss, now) {
        return None;
    }

    // A plugin picks its own size; the contract asks for its largest.
    let found = match Origin::of(source) {
        Origin::Subsonic => subsonic_cover(&accounts, source, key, FULL_COVER_SIZE),
        Origin::Plugin => crate::plugins::cover(source, key),
        Origin::Local | Origin::Radio | Origin::Unknown => None,
    };

    if let Ok(mut misses) = MISSES.lock() {
        match found {
            Some(_) => misses.forget(&miss),
            None => misses.note(&miss, now),
        }
    }

    found
}

/// Keyed with the source, since two plugins can hand out the same key.
/// Subsonic's thumbs keep the bare key so the stored ones stay valid.
fn plugin_thumb(thumbs: &Mutex<Connection>, source: &str, key: &str) -> Option<Vec<u8>> {
    let thumb = plugin_thumb_key(source, key);

    if let Some(stored) = rox_library::thumbs::thumbnail(thumbs, Path::new(&thumb)) {
        return Some(stored);
    }

    let bytes = crate::plugins::cover(source, key)?;
    let thumbs = thumbs.lock().ok()?;

    // Plugin art fills square tiles, and an artist's picture often isn't square.
    rox_library::thumbs::store_square(&thumbs, &bytes, &thumb)
}

pub fn plugin_thumb_key(source: &str, key: &str) -> String {
    format!("{source}|{key}")
}

/// A plugin track's cover whether or not it's a library row: the source
/// browser lists tracks long before any of them is picked. Blocking.
pub fn plugin_cover(thumbs: &Mutex<Connection>, source: &str, key: &str) -> Option<Vec<u8>> {
    let thumb = plugin_thumb_key(source, key);
    let now = Instant::now();
    if MISSES.lock().ok()?.recent(&thumb, now) {
        return None;
    }

    let found = plugin_thumb(thumbs, source, key);

    if let Ok(mut misses) = MISSES.lock() {
        match found {
            Some(_) => misses.forget(&thumb),
            None => misses.note(&thumb, now),
        }
    }

    found
}

/// In memory only: a stored miss would outlive the outage that caused it.
struct Misses(HashMap<String, Instant>);

static MISSES: LazyLock<Mutex<Misses>> = LazyLock::new(|| Mutex::new(Misses(HashMap::new())));

impl Misses {
    fn recent(&self, key: &str, now: Instant) -> bool {
        self.0
            .get(key)
            .is_some_and(|at| now.saturating_duration_since(*at) < RETRY_AFTER)
    }

    fn note(&mut self, key: &str, now: Instant) {
        if self.0.len() >= MISSES_SWEEP {
            self.0
                .retain(|_, at| now.saturating_duration_since(*at) < RETRY_AFTER);
        }

        self.0.insert(key.to_string(), now);
    }

    fn forget(&mut self, key: &str) {
        self.0.remove(key);
    }
}

/// Empty fields are honest: Subsonic reports no sort names, ReplayGain,
/// tempo, or sample format.
fn row_for(track: &SourceTrack, now: i64) -> TrackRow {
    TrackRow {
        // The song id stands in for a path; `id_for_path` resolves it back.
        path: track.id.clone(),
        sub: 0,
        cue: None,
        remote_url: track.stream_url.clone(),
        remote_live: false,
        title: track.title.clone(),
        artist: track.artist.clone(),
        album_artist: track.album_artist.clone(),
        album: track.album.clone(),
        title_sort: String::new(),
        artist_sort: String::new(),
        album_artist_sort: String::new(),
        album_sort: String::new(),
        genre: track.genre.clone(),
        year: track.year,
        disc_no: track.disc_no,
        track_no: track.track_no,
        duration_ms: track.duration_ms,
        codec: track.codec.clone(),
        bitrate_kbps: track.bitrate_kbps,
        sample_rate_hz: 0,
        bit_depth: 0,
        rating: 0,
        replay_gain: ReplayGain::default(),
        bpm: None,
        size: track.size.max(0) as u64,
        // No file to stat; scans only ever walk local roots anyway.
        mtime: now,
    }
}

pub fn row_count(conn: &Connection, source: &str) -> usize {
    store::sources(conn)
        .unwrap_or_default()
        .into_iter()
        .find(|(name, _)| name == source)
        .map(|(_, count)| count)
        .unwrap_or(0)
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> SourceTrack {
        SourceTrack {
            id: "sg-1".into(),
            title: "Jynweythek".into(),
            artist: "Aphex Twin".into(),
            album_artist: "Aphex Twin".into(),
            album: "Drukqs".into(),
            genre: "Electronic".into(),
            year: 2001,
            disc_no: 1,
            track_no: 1,
            duration_ms: 109_000,
            codec: "flac".into(),
            bitrate_kbps: 533,
            size: 7_261_184,
            stream_url: "https://music.example.com/rest/stream.view?id=sg-1&format=raw".into(),
            cover_id: "al-1".into(),
        }
    }

    #[test]
    fn a_song_becomes_a_row_keyed_on_its_server_id() {
        let row = row_for(&track(), 1_700_000_000);

        assert_eq!(row.path, "sg-1");
        assert_eq!(row.sub, 0);
        assert_eq!(row.title, "Jynweythek");
        assert_eq!(row.album_artist, "Aphex Twin");
        assert_eq!(row.year, 2001);
        assert_eq!(row.disc_no, 1);
        assert_eq!(row.duration_ms, 109_000);
        assert_eq!(row.codec, "flac");
        assert_eq!(row.size, 7_261_184);
        assert_eq!(row.mtime, 1_700_000_000);

        assert_eq!(
            row.remote_url,
            "https://music.example.com/rest/stream.view?id=sg-1&format=raw"
        );
        assert!(!row.remote_url.contains("&t="));
        assert!(!row.remote_live);
    }

    #[test]
    fn a_sparse_song_maps_the_way_an_untagged_file_would() {
        let mut sparse = track();
        sparse.year = 0;
        sparse.disc_no = 0;
        sparse.genre = String::new();
        sparse.bitrate_kbps = 0;

        let row = row_for(&sparse, 0);

        assert_eq!(row.year, 0);
        assert_eq!(row.disc_no, 0);
        assert_eq!(row.genre, "");
        assert_eq!(row.bitrate_kbps, 0);

        assert!(!row.replay_gain.any());
        assert!(row.bpm.is_none());
        assert_eq!(row.sample_rate_hz, 0);
        assert_eq!(row.bit_depth, 0);
        assert!(row.cue.is_none());
    }

    #[test]
    fn a_server_station_keeps_its_url_as_the_identity() {
        let station = station_for(&SourceStation {
            id: "ir-2".into(),
            name: "HBR1.com - Dream Factory".into(),
            stream_url: "http://ubuntu.hbr1.com:19800/ambient.aac".into(),
            home_page: "http://www.hbr1.com/".into(),
        });

        assert_eq!(station.url, "http://ubuntu.hbr1.com:19800/ambient.aac");
        assert_eq!(station.name, "HBR1.com - Dream Factory");
        assert!(station.genre.is_empty());
    }

    #[test]
    fn a_negative_size_reads_as_nothing_rather_than_wrapping() {
        let mut odd = track();
        odd.size = -1;

        assert_eq!(row_for(&odd, 0).size, 0);
    }

    #[test]
    fn the_prefix_is_the_one_a_server_files_under() {
        let server = Server::new("https://music.example.com", "andrew", "pw");

        assert!(server.source_id().starts_with(SOURCE_PREFIX));
    }

    fn account(enabled: bool, url: &str) -> SubsonicAccount {
        SubsonicAccount {
            enabled,
            url: url.to_string(),
            user: "andrew".into(),
            password: "pw".into(),
            ..SubsonicAccount::default()
        }
    }

    fn id(url: &str) -> String {
        source_of(&account(true, url)).expect("an address")
    }

    fn plugin(id: &str, enabled: bool) -> PluginRecord {
        PluginRecord {
            id: id.to_string(),
            enabled,
            label: "Demo".into(),
            ..PluginRecord::default()
        }
    }

    fn state(servers: &[SubsonicAccount], plugins: &[PluginRecord]) -> AccountsState {
        let mut state = AccountsState::default();
        state.subsonic_servers = servers.to_vec();
        state.plugins = plugins.to_vec();
        state
    }

    fn servers(servers: &[SubsonicAccount]) -> AccountsState {
        state(servers, &[])
    }

    const HOME: &str = "https://home.example.com";
    const WORK: &str = "https://work.example.com";

    fn library(sources: &[&str]) -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        store::init_schema(&conn).unwrap();

        let now = 1_700_000_000;
        let mut local = row_for(&track(), now);
        local.path = "/music/aphex/01.flac".into();
        local.remote_url = String::new();
        store::upsert_source_rows(&mut conn, "local", &[local]).unwrap();

        for source in sources {
            let rows = ["sg-1", "sg-2"].map(|id| {
                let mut song = track();
                song.id = id.into();
                row_for(&song, now)
            });
            store::upsert_source_rows(&mut conn, source, &rows).unwrap();
        }

        conn
    }

    fn count(conn: &Connection, source: &str) -> usize {
        row_count(conn, source)
    }

    #[test]
    fn a_sync_drops_the_rows_the_old_address_left_behind() {
        let home = id(HOME);
        let mut conn = library(&[&home, "subsonic:old"]);

        let mut kept = track();
        kept.id = "sg-1".into();
        let rows = [row_for(&kept, 1_700_000_100)];
        let keep = HashSet::from(["sg-1".to_string()]);

        let (pruned, departed) = reconcile(
            &mut conn,
            &home,
            &rows,
            &keep,
            &servers(&[account(true, HOME)]),
        )
        .unwrap();

        assert_eq!(pruned, 1, "the song the server stopped listing");
        assert_eq!(departed, 2, "both rows of the address the account left");

        assert_eq!(count(&conn, &home), 1);
        assert_eq!(count(&conn, "subsonic:old"), 0);
        assert_eq!(count(&conn, "local"), 1, "a sync never reaches a local row");
    }

    #[test]
    fn a_sync_leaves_the_other_servers_alone() {
        let (home, work) = (id(HOME), id(WORK));
        let mut conn = library(&[&home, &work]);
        let accounts = servers(&[account(true, HOME), account(false, WORK)]);

        let rows = [row_for(&track(), 1_700_000_100)];
        let keep = HashSet::from(["sg-1".to_string()]);
        let (_, departed) = reconcile(&mut conn, &home, &rows, &keep, &accounts).unwrap();

        assert_eq!(departed, 0);
        assert_eq!(count(&conn, &work), 2);
    }

    #[test]
    fn another_source_is_left_alone() {
        let home = id(HOME);
        let mut conn = library(&[&home, "subsonic:old"]);
        let mut station = track();
        station.id = "ir-1".into();
        store::upsert_source_rows(&mut conn, "radio", &[row_for(&station, 0)]).unwrap();

        let kept = Kept {
            subsonic: Some(HashSet::from([home.clone()])),
            plugins: None,
        };
        assert_eq!(drop_departed(&mut conn, &kept), 2);

        assert_eq!(count(&conn, "radio"), 1);
        assert_eq!(count(&conn, "local"), 1);
        assert_eq!(count(&conn, &home), 2);
    }

    #[test]
    fn a_switched_off_server_keeps_its_rows() {
        let home = id(HOME);
        let mut conn = library(&[&home, "subsonic:old"]);

        assert_eq!(prune_for(&mut conn, &servers(&[account(false, HOME)])), 2);

        assert_eq!(count(&conn, &home), 2);
        assert_eq!(count(&conn, "subsonic:old"), 0);
    }

    #[test]
    fn an_unsure_list_prunes_nothing() {
        let mut conn = library(&[&id(HOME), "subsonic:old"]);

        assert_eq!(
            prune_for(
                &mut conn,
                &servers(&[account(true, HOME), account(true, "   ")])
            ),
            0
        );
        assert_eq!(prune_for(&mut conn, &servers(&[])), 0);

        assert_eq!(count(&conn, "subsonic:old"), 2);
    }

    #[test]
    fn removing_a_server_takes_its_rows_and_only_its() {
        let (home, work) = (id(HOME), id(WORK));
        let mut conn = library(&[&home, &work, "subsonic:old"]);

        let gone = remove_rows(&mut conn, Some(&home), &servers(&[account(false, WORK)]));

        assert_eq!(gone, 4, "its own two and the two nobody names");
        assert_eq!(count(&conn, &home), 0);
        assert_eq!(count(&conn, "subsonic:old"), 0);
        assert_eq!(count(&conn, &work), 2);
        assert_eq!(count(&conn, "local"), 1);
    }

    #[test]
    fn removing_the_last_server_takes_every_subsonic_row() {
        let home = id(HOME);
        let mut conn = library(&[&home, "subsonic:old"]);

        assert_eq!(remove_rows(&mut conn, Some(&home), &servers(&[])), 4);

        assert_eq!(count(&conn, &home), 0);
        assert_eq!(count(&conn, "subsonic:old"), 0);
        assert_eq!(count(&conn, "local"), 1);
    }

    #[test]
    fn only_switched_on_servers_browse() {
        let (home, work) = (id(HOME), id(WORK));
        let live = live_ids_in(
            &servers(&[account(true, HOME), account(false, WORK)]),
            &|_| true,
        );

        assert!(!hides(&live, &home));
        assert!(hides(&live, &work));
        assert!(hides(&live, "subsonic:old"));

        for source in ["local", "radio"] {
            assert!(!hides(&live, source));
            assert!(!hides(&HashSet::new(), source));
        }
    }

    #[test]
    fn a_switched_off_plugin_hides_its_rows_and_keeps_them() {
        let mut conn = library(&["plugin:demo"]);

        let off = state(&[], &[plugin("demo", false)]);
        assert!(hides(&live_ids_in(&off, &|_| true), "plugin:demo"));

        assert_eq!(prune_for(&mut conn, &off), 0);
        assert_eq!(count(&conn, "plugin:demo"), 2);

        let on = state(&[], &[plugin("demo", true)]);
        assert!(!hides(&live_ids_in(&on, &|_| true), "plugin:demo"));
    }

    #[test]
    fn a_plugin_with_no_record_hides_and_departs() {
        let mut conn = library(&["plugin:demo", "plugin:other"]);
        let left = state(&[], &[plugin("other", true)]);

        assert!(hides(&live_ids_in(&left, &|_| true), "plugin:demo"));

        assert_eq!(prune_for(&mut conn, &left), 2);
        assert_eq!(count(&conn, "plugin:demo"), 0);
        assert_eq!(count(&conn, "plugin:other"), 2);
        assert_eq!(count(&conn, "local"), 1);
    }

    #[test]
    fn a_missing_plugins_rows_hide_and_stay() {
        let mut conn = library(&["plugin:demo"]);
        let on = state(&[], &[plugin("demo", true)]);

        assert!(hides(&live_ids_in(&on, &|_| false), "plugin:demo"));
        assert!(!hides(&live_ids_in(&on, &|id| id == "demo"), "plugin:demo"));

        assert_eq!(prune_for(&mut conn, &on), 0);
        assert_eq!(
            depart_once(&mut conn, &on, &AtomicBool::new(false)),
            0,
            "deleting the folder is how many people update a plugin"
        );
        assert_eq!(count(&conn, "plugin:demo"), 2);
    }

    #[test]
    fn a_departing_plugin_takes_its_membership_with_its_rows() {
        let mut conn = library(&[]);
        let tracks: Vec<members::PluginTrack> = ["a", "b"]
            .map(|key| members::PluginTrack {
                key: key.into(),
                title: key.into(),
                ..Default::default()
            })
            .into();
        members::set_collection(&mut conn, "plugin:demo", "liked", &tracks).unwrap();
        members::pick(&mut conn, "plugin:demo", &tracks[..1]).unwrap();
        members::pick(&mut conn, "plugin:other", &tracks[..1]).unwrap();

        let left = state(&[], &[plugin("other", true)]);
        assert_eq!(prune_for(&mut conn, &left), 2);

        assert_eq!(count(&conn, "plugin:demo"), 0);
        assert!(
            members::collections(&conn, "plugin:demo")
                .unwrap()
                .is_empty()
        );
        assert_eq!(count(&conn, "plugin:other"), 1);
        assert_eq!(
            members::collections(&conn, "plugin:other").unwrap().len(),
            1
        );
    }

    #[test]
    fn a_record_removed_while_closed_departs_on_the_next_start() {
        let mut conn = library(&["plugin:demo", "plugin:other"]);
        let done = AtomicBool::new(false);

        assert_eq!(
            depart_once(&mut conn, &state(&[], &[plugin("other", true)]), &done),
            2
        );
        assert_eq!(count(&conn, "plugin:demo"), 0);
        assert_eq!(count(&conn, "plugin:other"), 2);

        // Once per launch: a second library in the same run leaves it be.
        let mut again = library(&["plugin:demo"]);
        assert_eq!(
            depart_once(&mut again, &state(&[], &[plugin("other", true)]), &done),
            0
        );
        assert_eq!(count(&again, "plugin:demo"), 2);
    }

    #[test]
    fn no_records_at_all_departs_nothing_under_plugins() {
        let mut conn = library(&["plugin:demo"]);

        assert_eq!(
            depart_once(&mut conn, &state(&[], &[]), &AtomicBool::new(false)),
            0
        );
        assert_eq!(count(&conn, "plugin:demo"), 2);
    }

    #[test]
    fn a_disabled_records_rows_stay_through_the_start() {
        let mut conn = library(&["plugin:demo"]);

        let left = state(&[], &[plugin("demo", false)]);
        assert_eq!(depart_once(&mut conn, &left, &AtomicBool::new(false)), 0);
        assert_eq!(count(&conn, "plugin:demo"), 2);
    }

    #[test]
    fn no_plugin_records_prunes_no_plugin_rows() {
        let home = id(HOME);
        let mut conn = library(&[&home, "plugin:demo"]);

        assert_eq!(prune_for(&mut conn, &servers(&[account(true, HOME)])), 0);
        assert_eq!(remove_rows(&mut conn, Some(&home), &servers(&[])), 2);

        assert_eq!(count(&conn, "plugin:demo"), 2);
    }

    #[test]
    fn plugin_records_leave_subsonic_rows_to_the_accounts() {
        let mut conn = library(&["subsonic:old", "plugin:demo"]);

        assert_eq!(
            prune_for(&mut conn, &state(&[], &[plugin("demo", true)])),
            0
        );

        assert_eq!(count(&conn, "subsonic:old"), 2);
    }

    #[test]
    fn every_plugin_record_names_its_rows() {
        let named = labels(&state(&[], &[plugin("demo", true), plugin("off", false)]));

        assert_eq!(named.get("plugin:demo").map(String::as_str), Some("Demo"));
        assert_eq!(named.get("plugin:off").map(String::as_str), Some("Demo"));
        assert!(!named.contains_key("plugin:gone"));
    }

    #[test]
    fn a_missed_cover_waits_out_the_retry_window() {
        let mut misses = Misses(HashMap::new());
        let then = Instant::now();

        assert!(!misses.recent("sg-1", then));

        misses.note("sg-1", then);

        assert!(misses.recent("sg-1", then + Duration::from_secs(30)));
        assert!(!misses.recent("sg-2", then));

        assert!(!misses.recent("sg-1", then + RETRY_AFTER));

        misses.forget("sg-1");
        assert!(!misses.recent("sg-1", then));
    }

    #[test]
    fn a_crowded_miss_list_sheds_only_what_expired() {
        let mut misses = Misses(HashMap::new());
        let then = Instant::now();

        for n in 0..MISSES_SWEEP {
            misses.note(&format!("old-{n}"), then);
        }

        let later = then + RETRY_AFTER;
        misses.note("fresh", later);

        assert_eq!(misses.0.len(), 1);
        assert!(misses.recent("fresh", later));
    }
}
