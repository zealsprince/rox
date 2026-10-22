//! ADR 14's lookups with the switched-on plugin providers joined in, and the
//! one place a caller asks for lyrics or tag candidates. rox-net runs the
//! built-ins and ranks every candidate with one scorer, but it sits below
//! rox-plugins and can't reach a plugin, so the plugin half is gathered here
//! and handed down. The lookups are blocking.

use std::borrow::Cow;

use rox_net::providers::{
    self, LyricsCandidate, LyricsProvider, MetadataCandidate, MetadataProvider, TrackQuery,
};
use rox_plugins::manifest::ProviderDomain;

use crate::plugins::{self, PluginProvider};

struct PluginLyricsProvider(PluginProvider);

impl LyricsProvider for PluginLyricsProvider {
    fn name(&self) -> Cow<'static, str> {
        Cow::Owned(self.0.label.clone())
    }

    fn search(&self, query: &TrackQuery) -> Result<Vec<LyricsCandidate>, String> {
        plugins::provider_lyrics(&self.0.source, query)
    }
}

struct PluginMetadataProvider(PluginProvider);

impl MetadataProvider for PluginMetadataProvider {
    fn name(&self) -> Cow<'static, str> {
        Cow::Owned(self.0.label.clone())
    }

    fn search(&self, query: &TrackQuery) -> Result<Vec<MetadataCandidate>, String> {
        plugins::provider_metadata(&self.0.source, query)
    }
}

pub fn search_lyrics(query: &TrackQuery) -> Result<Vec<LyricsCandidate>, String> {
    let plugins: Vec<PluginLyricsProvider> = plugins::providers_on(ProviderDomain::Lyrics)
        .into_iter()
        .map(PluginLyricsProvider)
        .collect();
    let extra: Vec<&dyn LyricsProvider> = plugins.iter().map(|p| p as _).collect();

    providers::search_lyrics(query, &extra)
}

pub fn search_metadata(query: &TrackQuery) -> Result<Vec<MetadataCandidate>, String> {
    let plugins: Vec<PluginMetadataProvider> = plugins::providers_on(ProviderDomain::Metadata)
        .into_iter()
        .map(PluginMetadataProvider)
        .collect();
    let extra: Vec<&dyn MetadataProvider> = plugins.iter().map(|p| p as _).collect();

    providers::search_metadata(query, &extra)
}

/// Whether any lyrics lookup can answer: the online flag or a switched-on plugin.
/// Reads the settings file when the flag is off, so not per frame.
pub fn lyrics_available() -> bool {
    providers::lyrics_online() || !plugins::providers_on(ProviderDomain::Lyrics).is_empty()
}

/// Whether any tag lookup can answer: the online flag or a switched-on plugin.
/// Reads the settings file when the flag is off, so not per frame.
pub fn metadata_available() -> bool {
    providers::metadata_online() || !plugins::providers_on(ProviderDomain::Metadata).is_empty()
}
