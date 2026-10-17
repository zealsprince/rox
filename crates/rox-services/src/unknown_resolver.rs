//! Unknown rows in the play queue ([`rox_library::unknown`]). A play context
//! keeps them as placeholder entries, which the engine steps past without a
//! word, and this swaps each one for the plugin track a search finds before
//! it comes round. The swap is an insert after the placeholder and a remove
//! of it, the two edits ADR 16 allows, so the pool stays append-only. A miss,
//! or a placeholder the engine already stepped over, only goes.
//!
//! The search runs here and never on the decode thread: a plugin call can
//! take seconds, and the engine's commands wait behind an open. The Unknown
//! row's listens and heart move to the found track once it plays, not on the
//! hit, so a hit the listener skips past leaves the playlist alone (ADR 11).

use std::collections::{HashMap, HashSet};

use gpui::{App, Entity};

use rox_library::cue::TrackKey;

use crate::catalog::Library;
use crate::player::Player;
use crate::{plugins, unknown};

/// A queue entry as the resolver reads it off the snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Slot {
    pub id: u64,
    pub placeholder: bool,
}

/// A queue edit, in the order the engine applies them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Edit {
    InsertAfter(u64, TrackKey),
    Remove(u64),
}

/// What a Next does about the placeholders in its way.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Send these away, then skip as usual.
    Skip(Vec<u64>),
    /// Send these away, then hold until this one's search lands.
    Wait(Vec<u64>, u64),
}

/// A search's hit, and the Unknown row it stands for.
#[derive(Clone, Debug)]
pub(crate) struct Found {
    pub key: TrackKey,
    pub unknown_id: i64,
}

/// A Next held on a placeholder's search.
pub(crate) struct Waiting {
    pub id: u64,
    pub key: TrackKey,
    /// The entry playing when Next was pressed. Once the listener moves off
    /// it, the landing no longer skips.
    pub from: Option<u64>,
}

/// One search to run.
pub(crate) struct Job {
    pub run: u64,
    pub id: u64,
    pub key: TrackKey,
    pub source: Option<String>,
}

/// The session's placeholders. Entry ids restart with every session, so this
/// resets with it.
#[derive(Default)]
pub(crate) struct Placeholders {
    /// Bumped per session, so a search landing on a newer one is dropped.
    pub run: u64,
    /// Searched or searching, so a miss isn't asked again while the remove is
    /// on its way.
    pub asked: HashSet<u64>,
    /// Sent away. The snapshot shows them until the engine drains the remove.
    pub gone: HashSet<u64>,
    pub waiting: Option<Waiting>,
    /// Hits by placeholder key, so a song queued twice searches once.
    pub found: HashMap<TrackKey, Found>,
    /// Found tracks swapped in but not heard yet, and the Unknown row each
    /// one takes over when it is.
    pub adopting: HashMap<TrackKey, i64>,
}

impl Placeholders {
    pub fn reset(&mut self) {
        *self = Placeholders {
            run: self.run + 1,
            ..Default::default()
        };
    }
}

/// The picked plugin while it runs, else the Plugins page's default while
/// it runs, else the only one running. None when nothing runs, or several do
/// and nothing picked one.
pub fn source_for(
    picked: Option<&str>,
    default: Option<&str>,
    running: &[(String, String)],
) -> Option<String> {
    let runs = |source: &&str| running.iter().any(|(id, _)| id == source);
    if let Some(source) = picked.filter(runs).or(default.filter(runs)) {
        return Some(source.to_string());
    }

    match running {
        [(only, _)] => Some(only.clone()),
        _ => None,
    }
}

/// The edits a landed search makes. Only a placeholder still ahead of the
/// decode cursor takes the hit: one the engine reached was stepped over, and
/// an insert behind the cursor would be history nobody heard.
pub(crate) fn landing(
    order: &[Slot],
    cursor: usize,
    id: u64,
    found: Option<TrackKey>,
) -> Vec<Edit> {
    let Some(at) = order.iter().position(|slot| slot.id == id) else {
        return Vec::new();
    };

    match found {
        Some(key) if at > cursor => vec![Edit::InsertAfter(id, key), Edit::Remove(id)],
        _ => vec![Edit::Remove(id)],
    }
}

/// Placeholders at or behind the decode cursor. The engine stepped over
/// them, and a Prev onto one would fall forward to where it started.
pub(crate) fn passed(order: &[Slot], cursor: usize, gone: &HashSet<u64>) -> Vec<u64> {
    order
        .iter()
        .take(cursor + 1)
        .filter(|slot| slot.placeholder && !gone.contains(&slot.id))
        .map(|slot| slot.id)
        .collect()
}

/// The placeholder to search now: the entry the engine opens next, while
/// the one before it plays. Search and pick take longer than an open.
pub(crate) fn up_next(order: &[Slot], cursor: usize, gone: &HashSet<u64>) -> Option<u64> {
    order
        .iter()
        .skip(cursor + 1)
        .find(|slot| !gone.contains(&slot.id))
        .filter(|slot| slot.placeholder)
        .map(|slot| slot.id)
}

/// Walks a Next from the audible entry. A placeholder it meets holds the
/// skip for its search, except the one already held on: a second press
/// passes it, so a run of Unknown rows is walked one press or one search at
/// a time.
pub(crate) fn next_step(
    order: &[Slot],
    audible: Option<usize>,
    waiting: Option<u64>,
    gone: &HashSet<u64>,
) -> Step {
    let Some(audible) = audible else {
        return Step::Skip(Vec::new());
    };

    let mut skip = Vec::new();
    for slot in order.iter().skip(audible + 1) {
        if gone.contains(&slot.id) {
            continue;
        }

        if !slot.placeholder {
            break;
        }

        if waiting == Some(slot.id) {
            skip.push(slot.id);
            continue;
        }

        return Step::Wait(skip, slot.id);
    }

    Step::Skip(skip)
}

/// Searches each placeholder as it comes up, and hands the Unknown row over
/// once its found track plays. The pump notifies every tick while playing,
/// so this is gated on the queue, the playing entry, a held Next and the
/// plugin table moving.
pub fn follow(library: Entity<Library>, player: Entity<Player>, cx: &mut App) {
    let mut seen = None;

    cx.observe(&player, move |player, cx| {
        // Nothing runs before the first apply, so every search would miss.
        // The apply moves the generation, which brings this back round.
        if !plugins::applied() {
            return;
        }

        let at = {
            let player = player.read(cx);
            (
                player.queue_rev(),
                player.playing_entry(),
                player.placeholder_waiting(),
                plugins::generation(),
            )
        };
        if seen == Some(at) {
            return;
        }
        seen = Some(at);

        let (heard, jobs) = player.update(cx, |player, _| {
            (player.take_heard(), player.placeholder_jobs())
        });

        if let Some((unknown_id, key)) = heard {
            let adopted = unknown::adopt(library.clone(), unknown_id, key, cx);
            cx.spawn(async move |_| {
                if let Err(e) = adopted.await {
                    log::warn!("moving an unknown row onto its plugin track: {e}");
                }
            })
            .detach();
        }

        for job in jobs {
            search(&library, &player, job, cx);
        }
    })
    .detach();
}

fn search(library: &Entity<Library>, player: &Entity<Player>, job: Job, cx: &mut App) {
    let Job {
        run,
        id,
        key,
        source,
    } = job;

    if let Some(found) = player.read(cx).placeholder_found(&key) {
        player.update(cx, |player, cx| {
            player.land_placeholder(run, id, Some(found), cx)
        });
        return;
    }

    let row = library.read(cx).resolve_key(&key);
    let (Some((unknown_id, meta)), Some(source)) = (row, source) else {
        player.update(cx, |player, cx| player.land_placeholder(run, id, None, cx));
        return;
    };

    let searched = plugins::find_track(library.clone(), &source, meta.artist, meta.title, cx);
    let player = player.clone();
    cx.spawn(async move |cx| {
        let found = searched.await.unwrap_or_else(|e| {
            log::info!("{source}: searching for a queued unknown row failed: {e}");
            None
        });

        let found = found.map(|key| Found { key, unknown_id });
        player
            .update(cx, |player, cx| player.land_placeholder(run, id, found, cx))
            .ok();
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const A: u64 = 0;
    const P: u64 = 1;
    const B: u64 = 2;

    fn key(name: &str) -> TrackKey {
        TrackKey::from(PathBuf::from(format!("/m/{name}.flac")))
    }

    fn slots(order: &[(u64, bool)]) -> Vec<Slot> {
        order
            .iter()
            .map(|&(id, placeholder)| Slot { id, placeholder })
            .collect()
    }

    /// The engine's side of ADR 16's two edits, over entry names.
    fn apply(order: &mut Vec<(u64, String)>, edits: Vec<Edit>) {
        for edit in edits {
            match edit {
                Edit::InsertAfter(after, key) => {
                    let at = order.iter().position(|(id, _)| *id == after).unwrap() + 1;
                    let name = key.path.file_stem().unwrap().to_string_lossy().into_owned();
                    order.insert(at, (100, name));
                }

                Edit::Remove(id) => order.retain(|(entry, _)| *entry != id),
            }
        }
    }

    fn named(order: &[(u64, String)]) -> Vec<&str> {
        order.iter().map(|(_, name)| name.as_str()).collect()
    }

    fn queue() -> (Vec<Slot>, Vec<(u64, String)>) {
        let order = slots(&[(A, false), (P, true), (B, false)]);
        let names = vec![(A, "A".into()), (P, "P".into()), (B, "B".into())];
        (order, names)
    }

    #[test]
    fn a_hit_takes_the_placeholder_s_place() {
        let (order, mut names) = queue();

        apply(&mut names, landing(&order, 0, P, Some(key("F"))));
        assert_eq!(named(&names), ["A", "F", "B"]);
    }

    #[test]
    fn a_miss_only_takes_the_placeholder_out() {
        let (order, mut names) = queue();

        apply(&mut names, landing(&order, 0, P, None));
        assert_eq!(named(&names), ["A", "B"]);
    }

    #[test]
    fn a_hit_after_the_engine_passed_the_placeholder_does_not_splice() {
        let (order, mut names) = queue();

        // The engine stepped over P and opened B before the search landed.
        apply(&mut names, landing(&order, 2, P, Some(key("F"))));
        assert_eq!(named(&names), ["A", "B"]);
    }

    #[test]
    fn a_landing_on_a_placeholder_already_gone_changes_nothing() {
        let order = slots(&[(A, false), (B, false)]);

        assert!(landing(&order, 0, P, Some(key("F"))).is_empty());
    }

    #[test]
    fn placeholders_behind_the_cursor_go_and_the_next_one_is_searched() {
        let order = slots(&[(0, true), (1, false), (2, true), (3, true), (4, false)]);
        let mut gone = HashSet::new();

        assert_eq!(passed(&order, 1, &gone), [0]);
        assert_eq!(up_next(&order, 1, &gone), Some(2));
        assert_eq!(up_next(&order, 0, &gone), None, "a playable entry is next");

        gone.insert(2);
        assert_eq!(
            up_next(&order, 1, &gone),
            Some(3),
            "the remove is on its way"
        );
    }

    #[test]
    fn a_next_holds_on_a_placeholder_and_a_second_press_passes_it() {
        let order = slots(&[(A, false), (P, true), (3, true), (B, false)]);
        let gone = HashSet::new();

        assert_eq!(
            next_step(&order, Some(0), None, &gone),
            Step::Wait(vec![], P)
        );
        assert_eq!(
            next_step(&order, Some(0), Some(P), &gone),
            Step::Wait(vec![P], 3),
            "the run is walked one at a time"
        );
        assert_eq!(
            next_step(&order, Some(0), Some(3), &gone),
            Step::Wait(vec![], P)
        );

        let gone: HashSet<u64> = [P, 3].into();
        assert_eq!(next_step(&order, Some(0), None, &gone), Step::Skip(vec![]));
    }

    #[test]
    fn a_next_onto_a_playable_entry_skips_as_usual() {
        let order = slots(&[(A, false), (B, false), (P, true)]);
        let gone = HashSet::new();

        assert_eq!(next_step(&order, Some(0), None, &gone), Step::Skip(vec![]));
        assert_eq!(next_step(&order, None, None, &gone), Step::Skip(vec![]));
    }

    #[test]
    fn the_pick_wins_then_the_default_then_the_only_one_running() {
        let one = vec![("plugin:a".to_string(), "A".to_string())];
        let two = vec![
            ("plugin:a".to_string(), "A".to_string()),
            ("plugin:b".to_string(), "B".to_string()),
        ];

        assert_eq!(
            source_for(Some("plugin:b"), Some("plugin:a"), &two).as_deref(),
            Some("plugin:b")
        );
        assert_eq!(
            source_for(None, Some("plugin:b"), &two).as_deref(),
            Some("plugin:b")
        );
        assert_eq!(
            source_for(Some("plugin:c"), Some("plugin:b"), &two).as_deref(),
            Some("plugin:b"),
            "a stopped pick falls to the default"
        );
        assert_eq!(source_for(None, None, &one).as_deref(), Some("plugin:a"));
        assert_eq!(
            source_for(Some("plugin:b"), Some("plugin:b"), &one).as_deref(),
            Some("plugin:a"),
            "a stopped pick and default fall back"
        );
        assert_eq!(
            source_for(None, None, &two),
            None,
            "several and no pick skips"
        );
        assert_eq!(source_for(None, Some("plugin:c"), &two), None);
        assert_eq!(source_for(None, None, &[]), None);
    }
}
