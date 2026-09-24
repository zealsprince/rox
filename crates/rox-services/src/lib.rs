//! The headless service layer the app's shared state is made of: gpui
//! entities that own state and emit when it moves. None of them render or
//! know about panels, and nothing here calls back up into the binary: where a
//! service needs the app, it emits or takes a plain argument.

pub mod acoustic;
pub mod artists;
pub mod backdrop;
pub mod capture;
pub mod catalog;
pub mod cues;
pub mod discord_presence;
pub mod history;
pub mod lastfm;
pub mod librefm;
pub mod listenbrainz;
pub mod lyrics;
pub mod peaks;
pub mod player;
pub mod portraits;
pub mod radio;
pub mod radio_art;
pub mod release_facts;
pub mod selection;
pub mod sources;
pub mod sources_registry;
pub mod station_art;
pub mod thumbs;
pub mod track_stats;
