//! The broadcast sink's app side (ADR 22): the settings and the stream
//! metadata. rox-playback owns the encoder and the icecast connection.

use gpui::App;

use rox_library::cue::TrackKey;
use rox_panel_api::panel::AppState;
use rox_playback::broadcast;

/// Start, retune, or stop the sink to match the settings.
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

pub fn start(state: &AppState, cx: &mut App) {
    apply();
    let state = state.clone();
    let player = state.player.clone();
    let mut current: Option<TrackKey> = None;
    // A station relay changes song without the key moving, so the title
    // revision is compared too.
    let mut current_live: Option<u64> = None;
    cx.observe(&player, move |_, cx| {
        let player = state.player.read(cx);
        let now = player.now_playing().map(|now| now.key);
        let live = player.title_rev();
        if now == current && live == current_live {
            return;
        }
        current = now.clone();
        current_live = live;
        let Some(key) = now else { return };
        // Title, else filename, so the mount never announces an empty line.
        let tags = player.live_over(state.library.read(cx).meta_for_key(&key));
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
