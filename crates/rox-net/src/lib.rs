//! Everything rox sends over the wire, and the service identities a build
//! sends it as. Nothing here draws anything and every call blocks, so the
//! app runs them on its background executor.

pub mod discord;
pub mod lastfm;
pub mod librefm;
pub mod listenbrainz;
pub mod providers;
pub mod sources;
