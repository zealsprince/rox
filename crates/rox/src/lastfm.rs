//! The scrobbler and the favourites mirror live in
//! [`rox_services::lastfm`] now and are re-exported whole below, so every
//! `crate::lastfm::` path in the app reads the way it always did. The
//! loved-list import stays here: it's a task-window job with its own
//! progress, not something the scrobbler drives.

pub mod import;

pub use rox_services::lastfm::*;
