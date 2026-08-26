//! The broadcast sink's app side (ADR 22): rox-playback owns the encoder,
//! the icecast source connection, and the retry clock; this module owns
//! what only the app knows - the settings that configure it and the
//! library tags behind the stream metadata. The metadata rides the same
//! player observer the media widget publishes on, keyed to the track so a
//! steady stream of clock notifies writes nothing.

use gpui::App;

use rox_library::cue::TrackKey;
use rox_panel_api::panel::AppState;
use rox_playback::broadcast;

/// Point the sink at the current settings: start it, retune it, or tear it
/// down, whichever the file says. Startup calls it once; whatever edits
/// the broadcast settings calls it again to make the change live.
pub fn apply() {
    let s = rox_core::settings::Settings::load().broadcast;
    let config = s.enabled.then_some(broadcast::Config {
        host: s.host,
        port: s.port,
        mount: s.mount,
        user: s.user,
        password: s.password,
        name: s.name,
        bitrate: s.bitrate,
    });
    broadcast::configure(config);
}

/// Apply the configured sink and start feeding it metadata off the player
/// observer. App-level, once per process, beside the control socket.
pub fn start(state: &AppState, cx: &mut App) {
    apply();
    let state = state.clone();
    let player = state.player.clone();
    let mut current: Option<TrackKey> = None;
    cx.observe(&player, move |_, cx| {
        let now = state.player.read(cx).now_playing().map(|now| now.key);
        if now == current {
            return;
        }
        current = now.clone();
        let Some(key) = now else { return };
        // The same title-or-filename fallback the media widget shows, so
        // the mount never announces an empty line.
        let tags = state.library.read(cx).meta_for_key(&key);
        let title = tags
            .as_ref()
            .map(|t| t.title.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| {
                key.path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        let artist = tags.map(|t| t.artist).unwrap_or_default();
        broadcast::set_song(if artist.is_empty() {
            title
        } else {
            format!("{artist} - {title}")
        });
    })
    .detach();
}
