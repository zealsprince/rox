//! Hearts mirrored onto a plugin's service (ADR 30, amended 2026-10-05).
//! While Sync Favourites is on, a change to the library's favourite set runs
//! the add or remove action a plugin names under `favourites`, on that
//! plugin's own tracks. It diffs the set the way the Last.fm mirror does, so
//! every path that moves a heart is covered, and favourites from before the
//! switch went on are never pushed.
//!
//! The service's side is read off the rows' `favourite` flag, a hint for the
//! session and never a record. A heart whose two sides disagree draws half,
//! and [`toggle`] on it favourites on both.

use std::collections::{HashMap, HashSet, VecDeque};

use gpui::{App, Entity, Global};
use serde_json::Value;

use rox_core::settings;
use rox_library::cue::{PLUGIN_PREFIX, TrackKey};
use rox_plugins::manifest::{ActionDecl, FAVOURITE_FLAG, FavouritesDecl};

use crate::catalog::{Library, LibraryEvent};
use crate::plugin_actions::{self, JobEnd, Started};
use crate::plugins::running;

/// Tracks per call. A plugin that makes one request per track still answers
/// inside the listing timeout.
const BATCH: usize = 20;

#[derive(Default)]
struct Mirror {
    /// None until there's a set worth trusting: a snapshot before the
    /// library loaded would read as every favourite just taken back.
    seen: Option<HashSet<i64>>,
    /// One call in flight at a time, so a heart taken back straight after
    /// it was given can't reach the service first.
    queue: VecDeque<Push>,
    sending: bool,
}

impl Global for Mirror {}

struct Push {
    source: String,
    action: ActionDecl,
    keys: Vec<String>,
}

/// Follows one library's heart changes. Every workspace's library shares the
/// one snapshot, since they're all views of the same database and a change
/// made in one window would otherwise be sent again from the next.
pub fn follow(library: &Entity<Library>, cx: &mut App) {
    App::subscribe(cx, library, |library, event, cx| match event {
        LibraryEvent::PlaylistsChanged => mirror(&library, cx),
        // A rescan can rewrite the ids, so reseed without sending.
        LibraryEvent::Updated => seed(&library, cx),
        _ => {}
    })
    .detach();
}

/// Sync Favourites switched on or off. On starts from the hearts as they are.
pub fn set_enabled(library: &Entity<Library>, on: bool, cx: &mut App) {
    settings::set_plugin_favourites(on, cx);
    // Every heart's service side appears or goes with the switch.
    plugin_actions::bump_flags();
    seed(library, cx);

    if !on {
        cx.default_global::<Mirror>().queue.clear();
    }
}

/// Hearts the Last.fm import just wrote join the snapshot unsent. Called in
/// the same update as the write, ahead of the library event.
pub fn absorb(library: &Entity<Library>, cx: &mut App) {
    seed(library, cx);
}

/// Whether the track is in its plugin's service favourites. None unless
/// Sync Favourites is on, the plugin is running and declares favourites, and
/// it said something about this row.
pub fn remote(key: &TrackKey) -> Option<bool> {
    if !settings::plugin_favourites() || !key.source.starts_with(PLUGIN_PREFIX) {
        return None;
    }

    declared(&key.source)?;
    let flags = plugin_actions::flags_for(&key.source, key.path.to_str()?)?;

    Some(flags.iter().any(|flag| flag == FAVOURITE_FLAG))
}

/// A heart's click. A half heart favourites on both sides, a full one comes
/// off both, an empty one favourites.
pub fn toggle(library: &Entity<Library>, id: i64, cx: &mut App) {
    let (local, key) = {
        let library = library.read(cx);
        let key = library
            .keys_for(&[id])
            .ok()
            .and_then(|keys| keys.into_iter().next());
        (library.is_favourite(id), key)
    };

    // Already a heart here, so the library has nothing to diff: only the
    // service needs telling.
    if local && let Some(key) = key.filter(|key| remote(key) == Some(false)) {
        queue(&[key], true, cx);
        return;
    }

    library.update(cx, |library, cx| library.set_favourites(&[id], !local, cx));
}

fn seed(library: &Entity<Library>, cx: &mut App) {
    let seen = settings::plugin_favourites().then(|| library.read(cx).favourite_ids());
    cx.default_global::<Mirror>().seen = seen;
}

fn mirror(library: &Entity<Library>, cx: &mut App) {
    if !settings::plugin_favourites() {
        cx.default_global::<Mirror>().seen = None;
        return;
    }

    let now = library.read(cx).favourite_ids();
    let Some(before) = cx.default_global::<Mirror>().seen.replace(now.clone()) else {
        // First look since arming: the starting line, not a backlog.
        return;
    };

    let added: Vec<i64> = now.difference(&before).copied().collect();
    let removed: Vec<i64> = before.difference(&now).copied().collect();

    for (ids, on) in [(added, true), (removed, false)] {
        if ids.is_empty() {
            continue;
        }

        let keys = library.read(cx).keys_for(&ids).unwrap_or_default();
        queue(&keys, on, cx);
    }
}

/// Queues the add or remove for the plugin tracks among `keys`. One whose
/// service already agrees is skipped: a heart catching up to the service
/// has nothing to send.
fn queue(keys: &[TrackKey], on: bool, cx: &mut App) {
    let mut by_source: HashMap<&str, Vec<String>> = HashMap::new();
    for key in keys {
        if !key.source.starts_with(PLUGIN_PREFIX) || remote(key) == Some(on) {
            continue;
        }

        if let Some(path) = key.path.to_str() {
            by_source
                .entry(&key.source)
                .or_default()
                .push(path.to_string());
        }
    }

    let mirror = cx.default_global::<Mirror>();
    for (source, keys) in by_source {
        let Some(action) = action(source, on) else {
            continue;
        };

        for chunk in keys.chunks(BATCH) {
            mirror.queue.push_back(Push {
                source: source.to_string(),
                action: action.clone(),
                keys: chunk.to_vec(),
            });
        }
    }

    drain(cx);
}

fn drain(cx: &mut App) {
    let mirror = cx.default_global::<Mirror>();
    if mirror.sending {
        return;
    }
    let Some(push) = mirror.queue.pop_front() else {
        return;
    };
    mirror.sending = true;

    let task = plugin_actions::run(&push.source, &push.action, push.keys, Value::Null, cx);
    let source = push.source;

    cx.spawn(async move |cx| {
        // The answer's flags are what moves the heart, so a message has
        // nowhere to go and nobody to show it to.
        let failed = match task.await {
            Ok(Started::Done(_)) => None,

            Ok(Started::Job(job)) => {
                let Ok(watch) = cx.update(|cx| plugin_actions::watch(job, cx)) else {
                    return;
                };

                match watch.await {
                    JobEnd::Failed(e) => Some(e),
                    JobEnd::Finished(_) | JobEnd::Stopped => None,
                }
            }

            Err(e) => Some(e),
        };

        // The heart shows it half, and the next click tries again.
        if let Some(e) = failed {
            log::warn!("{source}: syncing favourites: {e}");
        }

        cx.update(|cx| {
            cx.default_global::<Mirror>().sending = false;
            cx.refresh_windows();
            drain(cx);
        })
        .ok();
    })
    .detach();
}

fn declared(source: &str) -> Option<FavouritesDecl> {
    let running = running(source)?;
    let cap = running.host.manifest().capabilities.source.as_ref()?;

    cap.favourites.clone()
}

fn action(source: &str, on: bool) -> Option<ActionDecl> {
    let declared = declared(source)?;
    let id = match on {
        true => declared.add,
        false => declared.remove,
    };

    plugin_actions::actions(source)
        .into_iter()
        .find(|action| action.id == id)
}
