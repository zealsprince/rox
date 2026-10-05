//! One track's favourite and rating, resolved once per track and held until
//! the library says they moved. Resolving costs queries, and a panel showing
//! a heart repaints far more often than the track changes, so each such
//! panel keeps a [`MarkCache`] and only resolves on a new key.

use gpui::SharedString;
use rox_design::assets::icons;
use rox_library::cue::TrackKey;
use rox_services::catalog::{Library, LibraryEvent};
use rox_services::{plugin_actions, plugin_favourites};

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Marks {
    /// None for a file the library doesn't know, which can't take a mark.
    pub id: Option<i64>,
    pub favourite: bool,
    /// The plugin service's favourite, while Sync Favourites mirrors the
    /// track there and the plugin has said.
    pub remote: Option<bool>,
    pub rating: u8,
}

impl Marks {
    /// Favourite on one side and not the other.
    pub fn half(&self) -> bool {
        self.remote.is_some_and(|remote| remote != self.favourite)
    }

    /// The heart's glyph, and whether it draws in the accent.
    pub fn heart(&self) -> (&'static str, bool) {
        match (self.half(), self.favourite) {
            (true, _) => (icons::HEART_HALF, true),
            (false, true) => (icons::HEART_FILLED, true),
            (false, false) => (icons::HEART, false),
        }
    }

    /// What a click on the heart does. Dimmed and unclickable reads as
    /// broken, so with no id it says there's no track under it.
    pub fn heart_tip(&self) -> SharedString {
        match (self.id.is_some(), self.half(), self.favourite) {
            (false, _, _) => rox_i18n::t!("transport-favourite-nothing"),
            (true, true, true) => rox_i18n::t!("transport-favourite-half-here"),
            (true, true, false) => rox_i18n::t!("transport-favourite-half-there"),
            (true, false, true) => rox_i18n::t!("transport-favourite-remove"),
            (true, false, false) => rox_i18n::t!("transport-favourite-add"),
        }
    }
}

#[derive(Default)]
pub struct MarkCache {
    held: Option<(TrackKey, Marks)>,
    /// [`plugin_actions::flags_moved`] when `remote` was read.
    flags_seen: u64,
}

impl MarkCache {
    pub fn resolve(&mut self, key: &TrackKey, library: &Library) -> Marks {
        let moved = plugin_actions::flags_moved();

        if let Some((held, marks)) = &mut self.held
            && held == key
        {
            if self.flags_seen != moved {
                self.flags_seen = moved;
                marks.remote = plugin_favourites::remote(key);
            }
            return *marks;
        }

        let id = library.id_for_key(key);
        let marks = Marks {
            id,
            favourite: id.is_some_and(|id| library.is_favourite(id)),
            remote: id.and_then(|_| plugin_favourites::remote(key)),
            rating: id.map_or(0, |id| rating(library, id)),
        };
        self.held = Some((key.clone(), marks));
        self.flags_seen = moved;

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
