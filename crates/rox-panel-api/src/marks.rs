//! One track's favourite and rating, resolved once per track and held until
//! the library says they moved. Resolving costs queries, and a panel showing
//! a heart repaints far more often than the track changes, so each such
//! panel keeps a [`MarkCache`] and only resolves on a new key.

use rox_library::cue::TrackKey;
use rox_services::catalog::{Library, LibraryEvent};

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Marks {
    /// None for a file the library doesn't know, which can't take a mark.
    pub id: Option<i64>,
    pub favourite: bool,
    pub rating: u8,
}

#[derive(Default)]
pub struct MarkCache {
    held: Option<(TrackKey, Marks)>,
}

impl MarkCache {
    pub fn resolve(&mut self, key: &TrackKey, library: &Library) -> Marks {
        if let Some((held, marks)) = &self.held
            && held == key
        {
            return *marks;
        }

        let id = library.id_for_key(key);
        let marks = Marks {
            id,
            favourite: id.is_some_and(|id| library.is_favourite(id)),
            rating: id.map_or(0, |id| rating(library, id)),
        };
        self.held = Some((key.clone(), marks));

        marks
    }

    pub fn clear(&mut self) {
        self.held = None;
    }

    /// Folds in a library event, true when the holder should repaint. A
    /// playlist edit or a star click keeps the id, so it re-reads one mark;
    /// a rescan can remap ids to paths, so it drops everything.
    pub fn apply(&mut self, event: &LibraryEvent, library: &Library) -> bool {
        match event {
            LibraryEvent::Updated => {
                self.held = None;
                true
            }

            LibraryEvent::PlaylistsChanged => {
                self.reread(|marks, id| marks.favourite = library.is_favourite(id))
            }

            LibraryEvent::Rated => self.reread(|marks, id| marks.rating = rating(library, id)),

            _ => false,
        }
    }

    fn reread(&mut self, read: impl FnOnce(&mut Marks, i64)) -> bool {
        let Some((_, marks)) = self.held.as_mut() else {
            return false;
        };
        let Some(id) = marks.id else {
            return false;
        };

        read(marks, id);
        true
    }
}

fn rating(library: &Library, id: i64) -> u8 {
    library.ratings_for(&[id]).get(&id).copied().unwrap_or(0)
}
