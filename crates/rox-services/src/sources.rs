//! Non-local sources, and the sync that turns one into library rows. The
//! first is Subsonic: a server the user runs, whose catalog rox reads over
//! HTTP and files under its own source id. Headless like everything else
//! here, so nothing in this module knows a panel exists.
//!
//! A sync is a reconcile rather than an import. The server is asked what it
//! has, every song upserts under `subsonic:<digest>`, and anything still
//! filed under that id the server no longer lists gets pruned. Both halves
//! are scoped to the source string, so a sync can never reach a local row
//! no matter what the server sends back.
//!
//! There can be several servers, each an account in settings with its own
//! switch, and each digests to its own source id, so two servers' catalogs
//! never share a row.
//!
//! The same scoping is what lets a re-pointed account clean up after
//! itself. This module owns every `subsonic:` id there is, so a row under
//! one that no configured account digests to now can only have come from
//! an address an account has since left, and it goes. Nothing else could
//! play it: the authorizer below refuses to sign for a source id no
//! switched-on account matches, so those rows would sit in the library
//! looking playable and skip with the server's complaint about a password
//! that was never the problem.
//!
//! The other half is the authorize table. A remote row stores its stream
//! URL and nothing else, because a credential stored in SQLite is a
//! credential somebody can lift back out of it. When playback resolves a
//! row it asks this module to finish the request and the live source object
//! answers from settings. Subsonic authorizes in the query string, so what
//! it adds is a fresh salt and token on the URL and no headers at all; the
//! table still takes headers because the next source will have some.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{App, Entity, Task};

use rox_core::settings::{Settings, SubsonicAccount};
use rox_library::TrackRow;
use rox_library::playlists;
use rox_library::replaygain::ReplayGain;
use rox_library::rusqlite::Connection;
use rox_library::stations::{self, Station};
use rox_library::store;
use rox_net::sources::subsonic::{Server, ServerInfo};
use rox_net::sources::{SourceStation, SourceTrack};

use crate::catalog::Library;
use crate::sources_registry;

/// How wide a cover the server is asked to scale to. The thumbnail store
/// downscales again on the way in, so this only has to beat a grid tile
/// and stay well short of pulling a full-resolution scan down.
const COVER_SIZE: u32 = 512;

/// How long a cover that came back empty is left alone before a paint may
/// ask the server again. Long enough that a server that's down isn't
/// hammered by every repaint, short enough that one back up shows its art
/// within the session.
const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

/// How many remembered misses before the expired ones are swept out.
const MISSES_SWEEP: usize = 4096;

/// What every Subsonic source id starts with, and the whole namespace this
/// module answers for. Checked against [`Server::source_id`] by a test, so
/// the two can't drift apart without the build saying so.
const SOURCE_PREFIX: &str = "subsonic:";

/// A running sync, as the settings row reads it. Atomics rather than an
/// entity and an event: the work is on the background executor, the reader
/// repaints on its own clock, and nothing else in the app cares.
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

/// What a finished sync did, for the line the settings section shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    /// Rows written, which is every song the server listed.
    pub tracks: usize,
    /// Rows dropped because the server no longer lists them.
    pub pruned: usize,
    /// Rows dropped because they belong to a server this account has left.
    /// Separate from `pruned`: one is the server's catalog shrinking, the
    /// other is the account having moved house.
    pub departed: usize,
    /// Server playlists created here for the first time.
    pub playlists: usize,
    /// Internet radio stations the server lists, written to the radio source.
    pub stations: usize,
}

/// Which server the running sync is walking, by source id. None when no
/// sync is running. The settings page reads it to put the album count on
/// the right server's line.
static SYNCING: Mutex<Option<String>> = Mutex::new(None);

/// Albums walked and albums to walk, while a sync runs. None when none is.
pub fn progress() -> Option<(usize, usize)> {
    if !PROGRESS.running.load(Ordering::Relaxed) {
        return None;
    }

    Some((
        PROGRESS.done.load(Ordering::Relaxed),
        PROGRESS.total.load(Ordering::Relaxed),
    ))
}

/// Whether a sync is in flight, so a second Sync Now doesn't start one on
/// top of the first. One at a time across every server: two would race
/// each other's prune.
pub fn syncing() -> bool {
    PROGRESS.running.load(Ordering::Relaxed)
}

/// The source id of the server a sync is walking right now.
pub fn syncing_source() -> Option<String> {
    SYNCING.lock().ok()?.clone()
}

/// The configured servers, in the order the settings page lists them.
fn accounts() -> Vec<SubsonicAccount> {
    Settings::load().accounts.subsonic_servers
}

/// An account as the server it describes, switched on or not. None while
/// it names no address, which is a server that isn't set up yet.
fn server_of(account: &SubsonicAccount) -> Option<Server> {
    if account.url.trim().is_empty() {
        return None;
    }

    Some(Server::new(&account.url, &account.user, &account.password))
}

/// The source id an account's rows are filed under, None while it names no
/// address. What the settings page counts a server's rows by.
pub fn source_of(account: &SubsonicAccount) -> Option<String> {
    server_of(account).map(|server| server.source_id())
}

/// The server at `index` in the settings list. A switched-off account
/// still answers here, because Connect has to work before the switch goes
/// on.
pub fn server(index: usize) -> Option<Server> {
    server_of(accounts().get(index)?)
}

/// The switched-on servers: the ones whose rows browse, play and have
/// covers fetched. Off means a server isn't used for any of that, so
/// everything that reaches one on the library's behalf asks here rather
/// than [`server`].
fn live_servers() -> Vec<Server> {
    accounts()
        .iter()
        .filter(|account| account.enabled)
        .filter_map(server_of)
        .collect()
}

/// The switched-on server whose rows are filed under `source`.
fn live_server(source: &str) -> Option<Server> {
    live_servers()
        .into_iter()
        .find(|server| server.source_id() == source)
}

/// Fill the library's source-name table from the accounts: "Local" for
/// the files, "Radio" for the stations, and each server by the name it was
/// given or its host. What every surface that shows or matches a source by
/// name reads, see [`rox_library::cue::source_label`]. Run on every
/// projection load, which is also when a renamed or added server would
/// first show up anywhere, and reads the settings once.
pub fn publish_labels() {
    let mut labels = HashMap::new();
    labels.insert(
        rox_library::cue::LOCAL.to_string(),
        rox_i18n::t!("metadata-source-local").to_string(),
    );
    labels.insert(
        stations::SOURCE.to_string(),
        rox_i18n::t!("metadata-source-radio").to_string(),
    );

    for account in accounts() {
        let Some(source) = source_of(&account) else {
            continue;
        };

        let label = account.label();
        if !label.is_empty() {
            labels.insert(source, label);
        }
    }

    rox_library::cue::set_source_labels(labels);
}

/// Put every configured server in the registry, under the source id its
/// rows are keyed by. Called at startup, before anything can resolve a row,
/// so a locator built during the first frame already has somewhere to ask.
/// Safe to call again after the settings change, which is how a re-pointed
/// or newly added server gets its row.
///
/// A server with no address has no source id to file under, and a locator
/// resolved for a row nobody signs for goes out bare, which is the right
/// answer.
pub fn install_registry() {
    for server in accounts().iter().filter_map(server_of) {
        let source = server.source_id();
        let mine = source.clone();

        sources_registry::install(
            &source,
            // Rebuilt from settings on each call rather than captured here,
            // so a changed password takes effect without reinstalling the
            // row. Only a switched-on account with this very id signs: a row
            // left behind by a re-pointed account, or one of a server that's
            // switched off, goes out bare and the server refuses it.
            Box::new(move |remote| {
                let Some(server) = live_server(&mine) else {
                    return;
                };

                remote.headers = server.stream_headers();
                // The salt is fresh per request, so the token can only go
                // on here. A row that stored one would be a replayable
                // credential sitting in the database, which is the whole
                // reason the stored URL stops short of it.
                remote.url = server.sign(&remote.url);
            }),
        );
    }
}

/// The source ids whose rows browse: every switched-on account's.
fn live_ids(accounts: &[SubsonicAccount]) -> HashSet<String> {
    accounts
        .iter()
        .filter(|account| account.enabled)
        .filter_map(source_of)
        .collect()
}

/// The source ids whose rows a prune keeps: every account's that names an
/// address, switched on or not, since a switched-off account keeps its
/// catalog by design.
///
/// None when the prune shouldn't run at all. An account with an empty
/// address is a half-finished edit or one somebody cleared, and either way
/// there's no telling which rows were its. No accounts at all is the same
/// answer: the one way to arrive there on purpose is [`remove`], which
/// takes its own rows, so a list that reads empty anywhere else is more
/// likely a file that didn't load than an instruction.
fn kept_ids(accounts: &[SubsonicAccount]) -> Option<HashSet<String>> {
    if accounts.is_empty() {
        return None;
    }

    accounts.iter().map(source_of).collect()
}

/// Drop every row filed under a Subsonic source id outside `kept`, and
/// answer how many went.
///
/// Each id goes through [`store::prune_source`] with nothing to keep, which
/// is the same delete a reconcile does one row at a time. What points at
/// those rows is left the way the ordinary prune leaves it: a playlist
/// entry, a listen and a thumbnail all outlive the track they name, and
/// teaching this path to chase them would make a re-pointed server tidier
/// than a server that dropped a track.
fn drop_departed(conn: &mut Connection, kept: &HashSet<String>) -> usize {
    let departed: Vec<String> = store::sources(conn)
        .unwrap_or_default()
        .into_iter()
        .map(|(source, _)| source)
        .filter(|source| source.starts_with(SOURCE_PREFIX) && !kept.contains(source))
        .collect();

    let nothing = HashSet::new();
    departed
        .iter()
        .filter_map(|source| store::prune_source(conn, source, &nothing).ok())
        .sum()
}

/// The prune as the accounts decide it: everything no account names any
/// more, when [`kept_ids`] says the list can be trusted, and otherwise
/// nothing at all.
fn prune_for(conn: &mut Connection, accounts: &[SubsonicAccount]) -> usize {
    let Some(kept) = kept_ids(accounts) else {
        return 0;
    };

    drop_departed(conn, &kept)
}

/// Bring the library in line with the accounts the settings now describe.
/// What the settings page calls when an address or login it just wrote has
/// been committed, and when a switch flips. The rows an old address left
/// behind go now rather than whenever somebody next presses Sync Now, and
/// the catalog is rebuilt either way: the switches and the addresses decide
/// which servers' rows browse, see [`hidden_sources`], and the projection
/// only learns that on a load.
///
/// Answers how many rows went, which is zero on every call but the one
/// right after an account moves.
pub fn follow_accounts(library: Entity<Library>, cx: &mut App) -> Task<usize> {
    // A sync is already doing this at the end of its own reconcile, and two
    // writers on one database is a busy error rather than a race worth
    // handling. Its own reload reads the switches as they stand by then.
    if syncing() {
        return Task::ready(0);
    }

    let accounts = accounts();
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

        // The projection is never patched in place: rebuilt from SQLite
        // and swapped whole, which is also where the hidden rows are
        // decided.
        library
            .update(cx, |library, cx| library.reload_projection(cx))
            .ok();

        gone
    })
}

/// Take one server out of rox: drop its account from the list and delete
/// every row it filed. What Remove Server runs, once its confirm is
/// answered. The switch is the way to keep a catalog while not using it;
/// this is the way to be rid of one.
///
/// Rows under an address the removed account had since left go too, as
/// long as the accounts that stay all name one, so a server removed after a
/// re-point doesn't leave its old catalog hidden forever. The rows go the
/// way a prune takes them: a playlist entry, a listen and a stored cover
/// outlive them the same as they outlive a track the server dropped.
/// Answers how many rows went.
pub fn remove(index: usize, library: Entity<Library>, cx: &mut App) -> Task<usize> {
    // The same reason the prune above waits: a sync is writing, and the
    // rows it's writing would land after the delete.
    if syncing() {
        return Task::ready(0);
    }

    let mut accounts = accounts();
    if index >= accounts.len() {
        return Task::ready(0);
    }

    let removed = accounts.remove(index);
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

/// [`remove`]'s database half: the removed account's own rows, then what no
/// account left in the list names, if they can all be trusted to. Unlike
/// the everyday prune an empty list is trusted here, because it's empty
/// because somebody just asked for exactly that.
fn remove_rows(conn: &mut Connection, source: Option<&str>, left: &[SubsonicAccount]) -> usize {
    let nothing = HashSet::new();
    let own = source
        .and_then(|source| store::prune_source(conn, source, &nothing).ok())
        .unwrap_or(0);

    let kept: Option<HashSet<String>> = left.iter().map(source_of).collect();
    let departed = kept.map_or(0, |kept| drop_departed(conn, &kept));

    own + departed
}

/// Which sources the catalog leaves out, as the projection load asks it:
/// every Subsonic id that isn't a switched-on account's. That covers a
/// server that's switched off, whose rows are kept and not used, and the
/// rows of an address an account has half left, an empty one mid-edit,
/// which nothing could sign anyway. The settings are read once here, not
/// per source asked.
pub fn hidden_sources() -> impl Fn(&str) -> bool {
    let live = live_ids(&accounts());

    move |source| hides(&live, source)
}

/// [`hidden_sources`]'s rule with the live ids handed in.
fn hides(live: &HashSet<String>, source: &str) -> bool {
    source.starts_with(SOURCE_PREFIX) && !live.contains(source)
}

/// Reach the server at `index` and report what it says about itself. What
/// its Connect button runs, off the UI thread.
pub fn ping(index: usize, cx: &App) -> Task<Result<ServerInfo, String>> {
    let server = server(index);

    cx.background_executor().spawn(async move {
        let server = server.ok_or_else(|| "no server configured".to_string())?;

        server.ping()
    })
}

/// Pull one server's whole catalog in and reconcile the library against
/// it. The work runs on the background executor on a connection of its
/// own, the way every other pass that writes the database does, and the
/// projection reloads once at the end rather than per album.
pub fn sync(
    index: usize,
    library: Entity<Library>,
    cx: &mut App,
) -> Task<Result<SyncOutcome, String>> {
    let Some(server) = server(index) else {
        return Task::ready(Err("no server configured".to_string()));
    };

    // One sync at a time. Two would race each other's prune, and the loser
    // would delete what the winner had just written.
    if PROGRESS.running.swap(true, Ordering::SeqCst) {
        return Task::ready(Err("a sync is already running".to_string()));
    }

    PROGRESS.done.store(0, Ordering::Relaxed);
    PROGRESS.total.store(0, Ordering::Relaxed);

    let source = server.source_id();
    if let Ok(mut syncing) = SYNCING.lock() {
        *syncing = Some(source.clone());
    }

    // The rows this writes land under whatever source id the account now
    // digests to, and the registry is otherwise only filled at startup and
    // when the settings change. A server re-pointed at another URL would
    // sync a catalog nothing knew how to authorize until the next launch,
    // so the table gets the current accounts before the rows do.
    install_registry();

    let db_path = library.read(cx).db_path();
    let accounts = accounts();

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

        // Rows moved, so the in-memory projection is stale until it's
        // rebuilt from SQLite and swapped whole.
        if outcome.is_ok() {
            // Stamped by source id rather than by position, since the list
            // can change under a sync that takes minutes.
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

/// The sync itself: blocking, and off any gpui context so it reads as one
/// piece. Fetch the catalog, write it, drop what's gone, then playlists.
/// `accounts` is the list as it stood when the sync started, which is what
/// decides which other servers' rows are still somebody's.
fn run(
    server: &Server,
    accounts: &[SubsonicAccount],
    conn: &mut Connection,
) -> Result<SyncOutcome, String> {
    let source = server.source_id();

    let tracks = server.catalog(|done, total| {
        PROGRESS.done.store(done, Ordering::Relaxed);
        PROGRESS.total.store(total, Ordering::Relaxed);
    })?;

    let now = now_secs();
    let rows: Vec<TrackRow> = tracks.iter().map(|track| row_for(track, now)).collect();

    // What the server still lists is what survives. Everything else under
    // this source id went away on the server's side, so it goes here too.
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

/// The database half of a sync, so a test can run it without a server:
/// write what the catalog holds, drop what it stopped holding, then drop
/// what belongs to an address no account names any more. Answers the two
/// counts in that order. The other servers' rows are left alone: each is
/// still some account's.
///
/// The departed pass runs here rather than only on the settings page
/// because an account can move without the settings window being open at
/// all, by way of a hand-edited `accounts.json`.
fn reconcile(
    conn: &mut Connection,
    source: &str,
    rows: &[TrackRow],
    keep: &HashSet<String>,
    accounts: &[SubsonicAccount],
) -> Result<(usize, usize), String> {
    store::upsert_source_rows(conn, source, rows).map_err(|e| e.to_string())?;

    let pruned = store::prune_source(conn, source, keep).map_err(|e| e.to_string())?;
    let departed = prune_for(conn, accounts);

    Ok((pruned, departed))
}

/// Bring the server's playlists across, creating each one the first time
/// it's seen. A playlist that already exists by name is left alone: rox
/// can't tell a stale import from a list somebody has since edited here,
/// and clobbering the edit is the worse of the two mistakes.
///
/// Playlist trouble doesn't fail the sync. The tracks are already in by
/// the time this runs, and a server that refuses `getPlaylists` has still
/// handed over a library.
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

        // Server ids to row ids, in the playlist's own order. A song the
        // catalog didn't return (unreadable on the server, filtered out of
        // a share) is skipped rather than failing the list.
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

/// The server's internet radio list, written as stations. They land in the
/// radio source beside the ones typed into the panel, keyed on the stream
/// URL like any station, so a re-sync updates a renamed one in place. What
/// this can't do is drop one the server removed: the radio list has no
/// memory of where a station came from, and pruning it would take the
/// typed ones with it. Removing is the panel's job.
///
/// Like playlists, trouble here doesn't fail the sync. A plain Subsonic
/// server without the endpoint has still handed over its library.
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

/// A server's station as the radio source stores it. The server's id is
/// dropped: the URL is the identity there, which is what lets the same
/// stream typed by hand and listed by the server be one row.
fn station_for(station: &SourceStation) -> Station {
    Station {
        url: station.stream_url.clone(),
        name: station.name.clone(),
        genre: String::new(),
    }
}

/// The picture for a row with no file behind it, by the string that stands
/// in for its path: a station's logo out of the thumbnail store, and a
/// server row's cover, fetched and filed the first time it's asked for.
/// Every surface that draws a remote row's art comes through here, so a
/// list tile, the cover panel and the OS media widget agree on the picture.
///
/// Blocking. Background executor only.
pub fn art(thumbs: &Mutex<Connection>, key: &str) -> Option<Vec<u8>> {
    rox_library::thumbs::thumbnail(thumbs, Path::new(key)).or_else(|| cover(thumbs, key))
}

/// One remote track's cover, fetched from the server and filed in the
/// thumbnail store under the song id, so the next ask is a lookup.
/// Deliberately not part of the sync: a library's worth of art is a
/// download nobody asked for, and the row that needs a picture is the one
/// on screen.
///
/// `key` is whatever the surface asked art by, which for a server row is
/// the song id standing in for a path. Only a key a switched-on server has a
/// row under goes anywhere near the server. Everything else that reaches
/// here (a station's URL, a local file deleted since its row was read) is
/// some other source's miss, and handing a local path to a server as a
/// song id would be telling it about the user's disk.
///
/// A key that came back with nothing is left alone for [`RETRY_AFTER`].
/// Every visible row re-asks each time the catalog moves, and without the
/// wait a server that's down would cost a timeout per row per change.
///
/// Blocking, and the store lock is only taken for the write, never across
/// the requests. Background executor only.
pub fn cover(thumbs: &Mutex<Connection>, key: &str) -> Option<Vec<u8>> {
    // A key that stats is a file, and a file's cover is the file's own
    // business, already answered by the store.
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

/// [`cover`]'s network half: which switched-on server has a row under the
/// key, which art id the song names there, and the image under it into the
/// store. The first server holding the key answers; two servers handing out
/// the same song id is a collision the thumbnail store, keyed on the id
/// alone, couldn't tell apart anyway.
///
/// The library has no column for the art id the catalog walk saw, so the
/// song is asked for it again. That's one small reply per cover, paid once:
/// after it, the store answers.
fn fetch_cover(thumbs: &Mutex<Connection>, key: &str) -> Option<Vec<u8>> {
    let servers = live_servers();
    if servers.is_empty() {
        return None;
    }

    let library = store::open(&rox_core::settings::data_dir().join("library.db")).ok()?;
    let server = servers.into_iter().find(|server| {
        matches!(
            store::id_for_path(&library, &server.source_id(), key),
            Ok(Some(_))
        )
    })?;

    let cover_id = server.cover_id(key).ok()?;
    if cover_id.is_empty() {
        return None;
    }

    let bytes = server.cover(&cover_id, COVER_SIZE).ok()?;
    let thumbs = thumbs.lock().ok()?;

    rox_library::thumbs::store_bytes(&thumbs, &bytes, key)
}

/// The keys [`cover`] asked about recently and got nothing for, and when.
/// In memory only: a restart is a fair moment to try again, and a stored
/// miss would outlive the server outage that caused it.
struct Misses(HashMap<String, Instant>);

static MISSES: LazyLock<Mutex<Misses>> = LazyLock::new(|| Mutex::new(Misses(HashMap::new())));

impl Misses {
    /// Whether `key` came back empty inside the retry window.
    fn recent(&self, key: &str, now: Instant) -> bool {
        self.0
            .get(key)
            .is_some_and(|at| now.saturating_duration_since(*at) < RETRY_AFTER)
    }

    fn note(&mut self, key: &str, now: Instant) {
        // Long sessions scroll past a lot of rows. Past a few thousand the
        // expired entries go, so the map holds the window and not the day.
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

/// One song from the server as a library row. The empty fields are the
/// honest answer rather than a placeholder: Subsonic reports no sort
/// names, no ReplayGain, no tempo and no sample format, so those sit the
/// way they would for a file whose tags carry none.
fn row_for(track: &SourceTrack, now: i64) -> TrackRow {
    TrackRow {
        // The server's song id stands in for a path. It's what identity is
        // keyed on for this source and what `id_for_path` resolves back.
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
        // There's no file to stat, so the sync's own clock stands in.
        // Nothing reads it to decide whether to re-read a file, since
        // scans only ever walk local roots.
        mtime: now,
    }
}

/// What the library holds for one source, for the settings readout. Zero
/// when the source has never synced.
pub fn row_count(conn: &Connection, source: &str) -> usize {
    store::sources(conn)
        .unwrap_or_default()
        .into_iter()
        .find(|(name, _)| name == source)
        .map(|(_, count)| count)
        .unwrap_or(0)
}

/// Wall clock in unix seconds, the stamp every write here shares.
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

        // The stream URL rides the row; the credentials never do.
        assert_eq!(
            row.remote_url,
            "https://music.example.com/rest/stream.view?id=sg-1&format=raw"
        );
        assert!(!row.remote_url.contains("&t="));
        assert!(!row.remote_live);
    }

    #[test]
    fn a_sparse_song_maps_the_way_an_untagged_file_would() {
        // No year, no disc, no genre, no bitrate: what real servers return
        // constantly.
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

        // Nothing invents a measurement the server never made.
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

    /// The prefix the departed prune matches on is the one real source ids
    /// carry. A digest that stopped starting with it would leave every
    /// stale row in place and nothing else would notice.
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

    /// The source id the account at `url` files its rows under.
    fn id(url: &str) -> String {
        source_of(&account(true, url)).expect("an address")
    }

    const HOME: &str = "https://home.example.com";
    const WORK: &str = "https://work.example.com";

    /// An in-memory library holding one local row and two rows under each
    /// of `sources`.
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

    /// The finding this prune exists for: after the address changes, the
    /// rows under the old id can't be signed by anybody, so a sync drops
    /// them. The synced id reconciles the way it always has, and neither
    /// half reaches a local row.
    #[test]
    fn a_sync_drops_the_rows_the_old_address_left_behind() {
        let home = id(HOME);
        let mut conn = library(&[&home, "subsonic:old"]);

        // The server still lists one of its two songs.
        let mut kept = track();
        kept.id = "sg-1".into();
        let rows = [row_for(&kept, 1_700_000_100)];
        let keep = HashSet::from(["sg-1".to_string()]);

        let (pruned, departed) =
            reconcile(&mut conn, &home, &rows, &keep, &[account(true, HOME)]).unwrap();

        assert_eq!(pruned, 1, "the song the server stopped listing");
        assert_eq!(departed, 2, "both rows of the address the account left");

        assert_eq!(count(&conn, &home), 1);
        assert_eq!(count(&conn, "subsonic:old"), 0);
        assert_eq!(count(&conn, "local"), 1, "a sync never reaches a local row");
    }

    /// Syncing one server leaves every other configured server's rows be,
    /// switched on or off: each is still somebody's catalog.
    #[test]
    fn a_sync_leaves_the_other_servers_alone() {
        let (home, work) = (id(HOME), id(WORK));
        let mut conn = library(&[&home, &work]);
        let accounts = [account(true, HOME), account(false, WORK)];

        let rows = [row_for(&track(), 1_700_000_100)];
        let keep = HashSet::from(["sg-1".to_string()]);
        let (_, departed) = reconcile(&mut conn, &home, &rows, &keep, &accounts).unwrap();

        assert_eq!(departed, 0);
        assert_eq!(count(&conn, &work), 2);
    }

    /// Pruning is scoped to this module's own namespace. Another source's
    /// rows look exactly as stale from here and are none of its business.
    #[test]
    fn another_source_is_left_alone() {
        let home = id(HOME);
        let mut conn = library(&[&home, "subsonic:old"]);
        let mut station = track();
        station.id = "ir-1".into();
        store::upsert_source_rows(&mut conn, "radio", &[row_for(&station, 0)]).unwrap();

        assert_eq!(drop_departed(&mut conn, &HashSet::from([home.clone()])), 2);

        assert_eq!(count(&conn, "radio"), 1);
        assert_eq!(count(&conn, "local"), 1);
        assert_eq!(count(&conn, &home), 2);
    }

    /// The switch off keeps the server's catalog, which is what the switch
    /// promises. What its account left behind at an older address still
    /// goes, since no account names that one.
    #[test]
    fn a_switched_off_server_keeps_its_rows() {
        let home = id(HOME);
        let mut conn = library(&[&home, "subsonic:old"]);

        assert_eq!(prune_for(&mut conn, &[account(false, HOME)]), 2);

        assert_eq!(count(&conn, &home), 2);
        assert_eq!(count(&conn, "subsonic:old"), 0);
    }

    /// A half-typed address is not an instruction to empty the library,
    /// and neither is a list that reads empty.
    #[test]
    fn an_unsure_list_prunes_nothing() {
        let mut conn = library(&[&id(HOME), "subsonic:old"]);

        assert_eq!(
            prune_for(&mut conn, &[account(true, HOME), account(true, "   ")]),
            0
        );
        assert_eq!(prune_for(&mut conn, &[]), 0);

        assert_eq!(count(&conn, "subsonic:old"), 2);
    }

    /// Removing a server takes its rows and whatever no remaining account
    /// names, and never another server's.
    #[test]
    fn removing_a_server_takes_its_rows_and_only_its() {
        let (home, work) = (id(HOME), id(WORK));
        let mut conn = library(&[&home, &work, "subsonic:old"]);

        let gone = remove_rows(&mut conn, Some(&home), &[account(false, WORK)]);

        assert_eq!(gone, 4, "its own two and the two nobody names");
        assert_eq!(count(&conn, &home), 0);
        assert_eq!(count(&conn, "subsonic:old"), 0);
        assert_eq!(count(&conn, &work), 2);
        assert_eq!(count(&conn, "local"), 1);
    }

    /// The last server removed takes every Subsonic row with it: the empty
    /// list is trusted here, because it's what was asked for.
    #[test]
    fn removing_the_last_server_takes_every_subsonic_row() {
        let home = id(HOME);
        let mut conn = library(&[&home, "subsonic:old"]);

        assert_eq!(remove_rows(&mut conn, Some(&home), &[]), 4);

        assert_eq!(count(&conn, &home), 0);
        assert_eq!(count(&conn, "subsonic:old"), 0);
        assert_eq!(count(&conn, "local"), 1);
    }

    /// Only a switched-on server's rows browse. One that's off, and an
    /// address nobody names, hide; the other sources never do.
    #[test]
    fn only_switched_on_servers_browse() {
        let (home, work) = (id(HOME), id(WORK));
        let live = live_ids(&[account(true, HOME), account(false, WORK)]);

        assert!(!hides(&live, &home));
        assert!(hides(&live, &work));
        assert!(hides(&live, "subsonic:old"));

        for source in ["local", "radio"] {
            assert!(!hides(&live, source));
            assert!(!hides(&HashSet::new(), source));
        }
    }

    #[test]
    fn a_missed_cover_waits_out_the_retry_window() {
        let mut misses = Misses(HashMap::new());
        let then = Instant::now();

        assert!(!misses.recent("sg-1", then));

        misses.note("sg-1", then);

        assert!(misses.recent("sg-1", then + Duration::from_secs(30)));
        assert!(!misses.recent("sg-2", then));

        // Past the window the key is worth asking about again, and a cover
        // that did land clears the mark at once.
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
