//! The headless service layer the app's shared state is made of: the
//! catalog over the library database, the playback service, the scrobbler
//! and the history recorder behind it, the cover and portrait caches, the
//! shared selection, the baked backdrop, and the Discord presence. Every
//! one of these is a gpui entity that owns some state and emits when it
//! moves; none of them render anything or refer to panels at all.
//!
//! Nothing here calls back up into the binary. Where a service used to
//! call into the app (the taskbar sampler, the acoustic pass, the folder
//! picker), it emits or takes a plain argument instead, and the app wires
//! the rest.

pub mod acoustic;
pub mod artists;
pub mod backdrop;
pub mod catalog;
pub mod discord_presence;
pub mod history;
pub mod lastfm;
pub mod lyrics;
pub mod peaks;
pub mod player;
pub mod portraits;
pub mod selection;
pub mod thumbs;
