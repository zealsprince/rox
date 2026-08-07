//! Everything rox sends over the wire, and the service identities a build
//! sends it as. The enrichment providers, the signed Last.fm calls, and the
//! keys baked in at compile time all live here. Nothing here draws anything
//! and every call blocks, so the app runs them on its background executor.

pub mod discord;
pub mod lastfm;
pub mod providers;
