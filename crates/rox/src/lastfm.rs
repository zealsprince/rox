//! The scrobbler and the favourites mirror live in
//! [`rox_services::lastfm`] now. The loved-list import stays here: it's a
//! task-window job with its own progress, not something the scrobbler
//! drives.

pub mod import;
