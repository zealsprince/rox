//! Undo for the queue: the steps the player records for every edit of the
//! play order, and the plan that turns a step into engine commands.
//!
//! The engine owns the order (ADR 16/17), so a step never copies it or
//! writes it back. It asks the engine for the same Insert, Remove and Move
//! any edit sends, which touch `order` and only append to the pool. The plan
//! runs against [`Order`], the snapshot as the player reads it, kept by the
//! engine's own rules so each command sees what the ones before it left.
//!
//! The playing entry is never removed: the engine refuses to, and a step that
//! would take it out keeps it where it is, so the playing track and its
//! position stay as they are.

use std::collections::{HashMap, HashSet};

use rox_library::cue::TrackKey;

use crate::undo;

/// An entry as a step names it. Entry ids are numbered per session and a
/// re-added entry gets a new one, so a ref carries its session and a step
/// only acts on refs the live order still holds. A re-add hands the step
/// stack a remap from the old ref to the new.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EntryRef {
    session: u64,
    id: u64,
}

impl EntryRef {
    pub fn new(session: u64, id: u64) -> EntryRef {
        EntryRef { session, id }
    }
}

/// What an entry plays and how it was queued: enough to queue it again.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Item {
    pub key: TrackKey,
    pub explicit: bool,
    pub group: Option<u64>,
}

/// An entry a restore puts back, after the entry that stood before it.
#[derive(Clone, PartialEq, Debug)]
pub struct Placed {
    /// None for the front of the order.
    after: Option<EntryRef>,
    was: EntryRef,
    item: Item,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Op {
    /// Take these entries out. The playing one stays.
    Remove(Vec<EntryRef>),
    /// Put entries back where they stood, in order.
    Restore(Vec<Placed>),
    /// Move an entry to just after another, or to the front when None.
    Move {
        entry: EntryRef,
        after: Option<EntryRef>,
    },
    /// Make the order `before`, the playing entry, then `after`: a replaced
    /// queue coming back around the track that plays now.
    Rebuild {
        before: Vec<(EntryRef, Item)>,
        after: Vec<(EntryRef, Item)>,
    },
}

impl Op {
    fn remap(&mut self, map: &HashMap<EntryRef, EntryRef>) {
        let swap = |entry: &mut EntryRef| {
            if let Some(&to) = map.get(entry) {
                *entry = to;
            }
        };

        match self {
            Op::Remove(entries) => entries.iter_mut().for_each(swap),

            Op::Restore(placed) => {
                for place in placed {
                    place.after.as_mut().map(swap);
                    swap(&mut place.was);
                }
            }

            Op::Move { entry, after } => {
                swap(entry);
                after.as_mut().map(swap);
            }

            Op::Rebuild { before, after } => {
                before
                    .iter_mut()
                    .chain(after.iter_mut())
                    .for_each(|(entry, _)| swap(entry));
            }
        }
    }
}

/// One undo step: ops applied in order.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct QueueStep {
    ops: Vec<Op>,
}

impl QueueStep {
    pub fn new(ops: Vec<Op>) -> QueueStep {
        QueueStep { ops }
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn into_ops(self) -> Vec<Op> {
        self.ops
    }

    pub fn remap(&mut self, map: &HashMap<EntryRef, EntryRef>) {
        for op in &mut self.ops {
            op.remap(map);
        }
    }
}

impl undo::Step for QueueStep {
    fn merge(mut self, older: Self) -> Self {
        self.ops.extend(older.ops);
        self
    }
}

/// A command for the engine, in its own terms.
#[derive(Clone, PartialEq, Debug)]
pub enum Edit {
    /// After `after`, or at the end when None or gone. One explicit flag for
    /// the batch, the first item's.
    Insert {
        after: Option<u64>,
        items: Vec<Item>,
    },
    Remove(Vec<u64>),
    /// To just after `after`, or the front when None.
    Move {
        id: u64,
        after: Option<u64>,
    },
}

/// What applying a step did: the commands to send, the step that takes it
/// back, and every ref it re-added under a new id.
#[derive(Debug, Default)]
pub struct Applied {
    pub edits: Vec<Edit>,
    pub undo: QueueStep,
    pub remap: HashMap<EntryRef, EntryRef>,
}

/// The play order a step plans against, entry ids in engine order.
#[derive(Clone, Debug)]
pub struct Order {
    session: u64,
    entries: Vec<(u64, Item)>,
    playing: Option<u64>,
    /// The id the engine gives the next inserted entry.
    next_id: u64,
}

/// Where a run of restored entries goes.
enum Spot {
    Front,
    After(u64),
    End,
}

impl Order {
    pub fn new(
        session: u64,
        entries: Vec<(u64, Item)>,
        playing: Option<u64>,
        next_id: u64,
    ) -> Order {
        Order {
            session,
            entries,
            playing,
            next_id,
        }
    }

    pub fn entries(&self) -> &[(u64, Item)] {
        &self.entries
    }

    pub fn playing(&self) -> Option<u64> {
        self.playing
    }

    pub fn entry(&self, id: u64) -> EntryRef {
        EntryRef::new(self.session, id)
    }

    /// The step that brings this order back around whatever plays later,
    /// taken before a replace. The entry playing now stays in it, as the
    /// history Prev reaches.
    pub fn rebuild_back(&self) -> Option<Op> {
        self.around_playing(true)
    }

    /// The order split around the playing entry, with or without it.
    fn around_playing(&self, with_playing: bool) -> Option<Op> {
        let at = self.position(self.playing?)?;
        let held = |slice: &[(u64, Item)]| {
            slice
                .iter()
                .map(|(id, item)| (self.entry(*id), item.clone()))
                .collect()
        };
        let before = match with_playing {
            true => &self.entries[..=at],
            false => &self.entries[..at],
        };

        Some(Op::Rebuild {
            before: held(before),
            after: held(&self.entries[at + 1..]),
        })
    }

    pub fn apply(&mut self, step: QueueStep) -> Applied {
        let mut ops = step.ops;
        let mut applied = Applied::default();
        let mut undos = Vec::new();

        for at in 0..ops.len() {
            let op = ops[at].clone();
            let (undo, map) = self.apply_op(op, &mut applied.edits);

            if !map.is_empty() {
                for later in &mut ops[at + 1..] {
                    later.remap(&map);
                }
                for to in applied.remap.values_mut() {
                    if let Some(&newer) = map.get(to) {
                        *to = newer;
                    }
                }
                applied.remap.extend(map);
            }

            undos.extend(undo);
        }

        // Taken back newest first.
        undos.reverse();
        applied.undo = QueueStep::new(undos);
        applied
    }

    fn apply_op(
        &mut self,
        op: Op,
        edits: &mut Vec<Edit>,
    ) -> (Option<Op>, HashMap<EntryRef, EntryRef>) {
        match op {
            Op::Remove(entries) => (self.remove_entries(&entries, edits), HashMap::new()),

            Op::Restore(placed) => self.restore(placed, edits),

            Op::Move { entry, after } => (self.move_entry(entry, after, edits), HashMap::new()),

            Op::Rebuild { before, after } => self.rebuild(before, after, edits),
        }
    }

    fn remove_entries(&mut self, entries: &[EntryRef], edits: &mut Vec<Edit>) -> Option<Op> {
        let ids: HashSet<u64> = entries
            .iter()
            .filter_map(|&entry| self.live(entry))
            .filter(|&id| Some(id) != self.playing)
            .collect();
        if ids.is_empty() {
            return None;
        }

        let mut placed = Vec::new();
        for (at, (id, item)) in self.entries.iter().enumerate() {
            if !ids.contains(id) {
                continue;
            }

            placed.push(Placed {
                after: at
                    .checked_sub(1)
                    .map(|before| self.entry(self.entries[before].0)),
                was: self.entry(*id),
                item: item.clone(),
            });
        }

        let ids: Vec<u64> = placed.iter().map(|place| place.was.id).collect();
        self.remove(ids, edits);
        Some(Op::Restore(placed))
    }

    fn restore(
        &mut self,
        placed: Vec<Placed>,
        edits: &mut Vec<Edit>,
    ) -> (Option<Op>, HashMap<EntryRef, EntryRef>) {
        let mut landed: HashMap<EntryRef, u64> = HashMap::new();
        let mut last: Option<u64> = None;
        let mut added = Vec::new();
        let mut at = 0;

        while at < placed.len() {
            let first = &placed[at];

            // An anchor gone since is replaced by the last entry put back,
            // then the playing one, so the rest of the step still lands.
            let spot = match first.after {
                None => Spot::Front,
                Some(anchor) => match landed.get(&anchor).copied().or(self.live(anchor)) {
                    Some(id) => Spot::After(id),
                    None => match last.or(self.playing) {
                        Some(id) => Spot::After(id),
                        None => Spot::End,
                    },
                },
            };

            // A run chains each entry after the one before it, all of one
            // flag, so it goes in as one insert.
            let mut end = at + 1;
            while end < placed.len()
                && placed[end].after == Some(placed[end - 1].was)
                && placed[end].item.explicit == first.item.explicit
            {
                end += 1;
            }

            let run = &placed[at..end];
            let items: Vec<Item> = run.iter().map(|place| place.item.clone()).collect();
            let ids = self.insert_at(spot, items, edits);

            for (place, &id) in run.iter().zip(&ids) {
                landed.insert(place.was, id);
                added.push(self.entry(id));
            }
            last = ids.last().copied().or(last);
            at = end;
        }

        let remap = landed
            .into_iter()
            .map(|(was, id)| (was, self.entry(id)))
            .collect();
        let undo = (!added.is_empty()).then_some(Op::Remove(added));
        (undo, remap)
    }

    fn move_entry(
        &mut self,
        entry: EntryRef,
        after: Option<EntryRef>,
        edits: &mut Vec<Edit>,
    ) -> Option<Op> {
        let id = self.live(entry)?;
        let to = match after {
            Some(after) => Some(self.live(after)?),
            None => None,
        };
        if to == Some(id) {
            return None;
        }

        let from = self.before(id);
        if from == to {
            return None;
        }

        self.move_to(id, to, edits);
        Some(Op::Move {
            entry,
            after: from.map(|from| self.entry(from)),
        })
    }

    fn rebuild(
        &mut self,
        before: Vec<(EntryRef, Item)>,
        after: Vec<(EntryRef, Item)>,
        edits: &mut Vec<Edit>,
    ) -> (Option<Op>, HashMap<EntryRef, EntryRef>) {
        let Some(playing) = self.playing else {
            return (None, HashMap::new());
        };
        let Some(undo) = self.around_playing(false) else {
            return (None, HashMap::new());
        };

        let others: Vec<u64> = self
            .entries
            .iter()
            .map(|(id, _)| *id)
            .filter(|&id| id != playing)
            .collect();
        self.remove(others, edits);

        let mut remap = HashMap::new();
        let mut put = |order: &mut Order, spot: Spot, held: &[(EntryRef, Item)]| {
            let mut spot = spot;
            for run in held.chunk_by(|a, b| a.1.explicit == b.1.explicit) {
                let items = run.iter().map(|(_, item)| item.clone()).collect();
                let ids = order.insert_at(spot, items, edits);
                for ((was, _), &id) in run.iter().zip(&ids) {
                    remap.insert(*was, order.entry(id));
                }
                spot = match ids.last() {
                    Some(&id) => Spot::After(id),
                    None => Spot::After(playing),
                };
            }
        };
        put(self, Spot::Front, &before);
        put(self, Spot::After(playing), &after);

        (Some(undo), remap)
    }

    /// The front takes two commands: the engine only inserts after an entry
    /// or at the end, and only a move reaches the front.
    fn insert_at(&mut self, spot: Spot, items: Vec<Item>, edits: &mut Vec<Edit>) -> Vec<u64> {
        if items.is_empty() {
            return Vec::new();
        }

        match spot {
            Spot::After(id) => self.insert(Some(id), items, edits),

            Spot::End => self.insert(None, items, edits),

            Spot::Front => {
                let mut items = items.into_iter();
                let Some(head) = items.next() else {
                    return Vec::new();
                };
                let first = self.insert(None, vec![head], edits);
                let Some(&first_id) = first.first() else {
                    return Vec::new();
                };
                self.move_to(first_id, None, edits);

                let rest: Vec<Item> = items.collect();
                let mut ids = vec![first_id];
                if !rest.is_empty() {
                    ids.extend(self.insert(Some(first_id), rest, edits));
                }
                ids
            }
        }
    }

    // ── The engine's rules ──

    fn insert(&mut self, after: Option<u64>, items: Vec<Item>, edits: &mut Vec<Edit>) -> Vec<u64> {
        let at = after
            .and_then(|id| self.position(id))
            .map(|p| p + 1)
            .unwrap_or(self.entries.len());
        let ids: Vec<u64> = (self.next_id..self.next_id + items.len() as u64).collect();
        self.next_id += items.len() as u64;

        let rows: Vec<(u64, Item)> = ids.iter().copied().zip(items.iter().cloned()).collect();
        self.entries.splice(at..at, rows);
        edits.push(Edit::Insert { after, items });
        ids
    }

    fn remove(&mut self, ids: Vec<u64>, edits: &mut Vec<Edit>) {
        let drop: HashSet<u64> = ids
            .iter()
            .copied()
            .filter(|&id| Some(id) != self.playing)
            .collect();
        if drop.is_empty() {
            return;
        }

        self.entries.retain(|(id, _)| !drop.contains(id));
        edits.push(Edit::Remove(
            ids.into_iter().filter(|id| drop.contains(id)).collect(),
        ));
    }

    fn move_to(&mut self, id: u64, after: Option<u64>, edits: &mut Vec<Edit>) {
        let Some(from) = self.position(id) else {
            return;
        };
        let row = self.entries.remove(from);
        let at = match after {
            Some(after) => self
                .position(after)
                .map(|p| p + 1)
                .unwrap_or(self.entries.len()),
            None => 0,
        };
        self.entries.insert(at, row);
        edits.push(Edit::Move { id, after });
    }

    // ── Lookups ──

    fn position(&self, id: u64) -> Option<usize> {
        self.entries.iter().position(|(entry, _)| *entry == id)
    }

    /// The id a ref names in this order, None for another session's or one
    /// that's gone.
    fn live(&self, entry: EntryRef) -> Option<u64> {
        (entry.session == self.session && self.position(entry.id).is_some()).then_some(entry.id)
    }

    fn before(&self, id: u64) -> Option<u64> {
        let at = self.position(id)?;
        at.checked_sub(1).map(|before| self.entries[before].0)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn item(name: &str) -> Item {
        Item {
            key: TrackKey::from(PathBuf::from(name)),
            explicit: true,
            group: None,
        }
    }

    fn context(name: &str) -> Item {
        Item {
            explicit: false,
            group: Some(7),
            ..item(name)
        }
    }

    /// An order of `names` numbered from zero, playing the one named.
    fn order(session: u64, names: &[&str], playing: &str) -> Order {
        let entries: Vec<(u64, Item)> = names
            .iter()
            .enumerate()
            .map(|(id, name)| (id as u64, item(name)))
            .collect();
        let playing = names.iter().position(|name| *name == playing);
        Order::new(
            session,
            entries,
            playing.map(|at| at as u64),
            names.len() as u64,
        )
    }

    fn names(order: &Order) -> Vec<String> {
        order
            .entries
            .iter()
            .map(|(_, item)| item.key.path.to_string_lossy().into_owned())
            .collect()
    }

    fn id_of(order: &Order, name: &str) -> EntryRef {
        let (id, _) = order
            .entries
            .iter()
            .find(|(_, item)| item.key.path.as_os_str() == name)
            .expect("in the order");
        order.entry(*id)
    }

    /// An edit as the player records it: applied, and its undo kept.
    fn edit(order: &mut Order, op: Op) -> QueueStep {
        order.apply(QueueStep::new(vec![op])).undo
    }

    #[test]
    fn a_removal_undoes_and_redoes_to_the_same_order() {
        let mut queue = order(1, &["p", "a", "b", "c", "d"], "p");
        let start = names(&queue);

        let b = id_of(&queue, "b");
        let c = id_of(&queue, "c");
        let undo = edit(&mut queue, Op::Remove(vec![b, c]));
        let removed = names(&queue);
        assert_eq!(removed, ["p", "a", "d"]);

        let back = queue.apply(undo);
        assert_eq!(names(&queue), start);
        // Each came back under a new id, and the remap says which.
        assert_eq!(back.remap.len(), 2);
        assert!(!back.remap.contains_key(&id_of(&queue, "b")));

        queue.apply(back.undo);
        assert_eq!(names(&queue), removed);
    }

    #[test]
    fn a_removal_restores_flags_groups_and_the_front() {
        let mut queue = order(1, &["h", "p", "a"], "p");
        queue.entries[0].1 = context("h");
        let start = queue.entries.clone();

        let h = id_of(&queue, "h");
        let undo = edit(&mut queue, Op::Remove(vec![h]));
        let back = queue.apply(undo);

        let items: Vec<&Item> = queue.entries.iter().map(|(_, item)| item).collect();
        let want: Vec<&Item> = start.iter().map(|(_, item)| item).collect();
        assert_eq!(items, want);
        // The front takes an insert and a move.
        assert!(matches!(
            back.edits.as_slice(),
            [
                Edit::Insert { after: None, .. },
                Edit::Move { after: None, .. }
            ]
        ));
    }

    #[test]
    fn an_add_undoes_and_redoes_in_place() {
        let mut queue = order(1, &["p", "a", "b"], "p");
        let start = names(&queue);

        // Play Next: the player inserts, then records the removal of what it added.
        let mut edits = Vec::new();
        let p = queue.playing().expect("playing");
        let ids = queue.insert(Some(p), vec![item("x"), item("y")], &mut edits);
        let added = names(&queue);
        let undo = QueueStep::new(vec![Op::Remove(
            ids.iter().map(|&id| queue.entry(id)).collect(),
        )]);

        let back = queue.apply(undo);
        assert_eq!(names(&queue), start);

        queue.apply(back.undo);
        assert_eq!(names(&queue), added);
    }

    #[test]
    fn a_move_undoes_and_redoes() {
        let mut queue = order(1, &["p", "a", "b", "c"], "p");
        let start = names(&queue);

        let a = id_of(&queue, "a");
        let c = id_of(&queue, "c");
        let undo = edit(
            &mut queue,
            Op::Move {
                entry: a,
                after: Some(c),
            },
        );
        let moved = names(&queue);
        assert_eq!(moved, ["p", "b", "c", "a"]);

        let back = queue.apply(undo);
        assert_eq!(names(&queue), start);

        queue.apply(back.undo);
        assert_eq!(names(&queue), moved);
    }

    #[test]
    fn two_merged_moves_go_back_to_where_the_first_started() {
        use crate::undo::Step;

        let mut queue = order(1, &["p", "a", "b", "c"], "p");
        let start = names(&queue);

        let a = id_of(&queue, "a");
        let b = id_of(&queue, "b");
        let c = id_of(&queue, "c");
        let first = edit(
            &mut queue,
            Op::Move {
                entry: a,
                after: Some(b),
            },
        );
        let second = edit(
            &mut queue,
            Op::Move {
                entry: a,
                after: Some(c),
            },
        );

        queue.apply(second.merge(first));
        assert_eq!(names(&queue), start);
    }

    #[test]
    fn a_replace_comes_back_around_the_track_that_plays_now() {
        // A queue, then an album played over it in a fresh session.
        let old = order(1, &["h", "x", "u1", "u2"], "x");
        let undo = QueueStep::new(vec![old.rebuild_back().expect("playing")]);

        let mut album = order(2, &["a0", "a1", "a2"], "a1");
        let playing = album.playing();

        let back = album.apply(undo);
        assert_eq!(names(&album), ["h", "x", "a1", "u1", "u2"]);
        assert_eq!(album.playing(), playing);
        assert_eq!(album.position(playing.expect("playing")), Some(2));
        // Every entry of the old session maps onto its new id.
        assert_eq!(back.remap.len(), 4);

        album.apply(back.undo);
        assert_eq!(names(&album), ["a0", "a1", "a2"]);
        assert_eq!(album.playing(), playing);
    }

    #[test]
    fn a_ref_from_another_session_or_gone_does_nothing() {
        let mut queue = order(2, &["p", "a", "b"], "p");
        let start = names(&queue);

        let stale = EntryRef::new(1, 1);
        let gone = EntryRef::new(2, 40);
        let applied = queue.apply(QueueStep::new(vec![
            Op::Remove(vec![stale, gone]),
            Op::Move {
                entry: stale,
                after: None,
            },
        ]));

        assert!(applied.edits.is_empty());
        assert!(applied.undo.is_empty());
        assert_eq!(names(&queue), start);
    }

    #[test]
    fn an_older_step_follows_a_re_added_entry_through_the_remap() {
        let mut queue = order(1, &["p", "a", "b", "c"], "p");
        let start = names(&queue);

        let a = id_of(&queue, "a");
        let c = id_of(&queue, "c");
        let mut moved = edit(
            &mut queue,
            Op::Move {
                entry: a,
                after: Some(c),
            },
        );
        let a = id_of(&queue, "a");
        let removed = edit(&mut queue, Op::Remove(vec![a]));

        // Undo the removal: `a` is back under a new id.
        let back = queue.apply(removed);
        assert_ne!(id_of(&queue, "a"), a);

        // Without the remap the move would name an entry that's gone.
        let mut unmapped = queue.clone();
        assert!(unmapped.apply(moved.clone()).edits.is_empty());

        moved.remap(&back.remap);
        queue.apply(moved);
        assert_eq!(names(&queue), start);
    }

    #[test]
    fn the_playing_entry_is_never_removed() {
        let mut queue = order(1, &["a", "p", "b"], "p");
        let p = id_of(&queue, "p");
        let a = id_of(&queue, "a");
        let b = id_of(&queue, "b");

        // Undoing a Play Now that added `a`, `p` and `b`: `p` plays on.
        let undo = edit(&mut queue, Op::Remove(vec![a, p, b]));
        assert_eq!(names(&queue), ["p"]);

        // The redo puts back only what went.
        let back = queue.apply(undo);
        assert_eq!(names(&queue), ["a", "p", "b"]);
        assert_eq!(
            back.undo,
            QueueStep::new(vec![Op::Remove(vec![
                id_of(&queue, "a"),
                id_of(&queue, "b"),
            ])])
        );
    }

    #[test]
    fn a_restore_whose_anchor_went_lands_after_the_playing_entry() {
        let mut queue = order(1, &["p", "a", "b"], "p");
        let b = id_of(&queue, "b");
        let undo = edit(&mut queue, Op::Remove(vec![b]));

        // Something outside the stack took `a`, the anchor, away.
        let a = id_of(&queue, "a");
        let mut edits = Vec::new();
        queue.remove(vec![a.id], &mut edits);

        queue.apply(undo);
        assert_eq!(names(&queue), ["p", "b"]);
    }
}
