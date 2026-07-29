//! Discord Rich Presence integration: publishes the now-playing track,
//! playback status (playing/paused), and elapsed timestamps over Discord IPC.
//!
//! Socket communication and reconnects run on a background thread to prevent
//! stalling the main GPUI thread or audio path.

use std::time::{SystemTime, UNIX_EPOCH};

use discord_rich_presence::activity::{Activity, ActivityType, Assets, Button, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use gpui::{Context, Entity, Subscription};
use log::{error, info, warn};

use crate::panels::library::Library;
use crate::player::Player;
use crate::settings::{DiscordSettings, Settings};

/// Discord Client Application ID for rox, injected at compile time via DISCORD_APP_ID env var.
const DISCORD_APP_ID: Option<&'static str> = option_env!("DISCORD_APP_ID");

/// Commands sent from the GPUI main thread to the background IPC worker loop.
pub enum DiscordCommand {
    UpdatePresence(Option<DiscordTrackState>),
    ClearPresence,
}

/// Snapshot of the currently playing track state sent over channel.
#[derive(Clone, Debug, PartialEq)]
pub struct DiscordTrackState {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub codec: String,
    pub bitrate_kbps: u16,
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    pub is_playing: bool,
    pub show_lastfm_button: bool,
    pub show_youtube_button: bool,
}

impl DiscordTrackState {
    /// Compare all track metadata fields except position_secs (which updates continuously).
    pub fn same_metadata(&self, other: &Self) -> bool {
        self.title == other.title
            && self.artist == other.artist
            && self.album == other.album
            && self.codec == other.codec
            && self.bitrate_kbps == other.bitrate_kbps
            && self.duration_secs == other.duration_secs
            && self.is_playing == other.is_playing
            && self.show_lastfm_button == other.show_lastfm_button
            && self.show_youtube_button == other.show_youtube_button
    }
}

pub struct DiscordPresence {
    player: Entity<Player>,
    library: Entity<Library>,
    config: DiscordSettings,
    sender: async_channel::Sender<DiscordCommand>,
    last_sent_track: Option<DiscordTrackState>,
    last_sent_time: Option<SystemTime>,
    last_sent_position: f64,
    _player_changed: Subscription,
}

impl DiscordPresence {
    pub fn new(
        player: &Entity<Player>,
        library: &Entity<Library>,
        cx: &mut Context<Self>,
    ) -> Self {
        let (tx, rx) = async_channel::bounded::<DiscordCommand>(16);

        // Spawn background task to manage IPC client connection and event loop
        cx.background_executor()
            .spawn(async move {
                Self::run_ipc_loop(rx).await;
            })
            .detach();

        // Observe player ticks on GPUI main thread
        let _player_changed = cx.observe(player, |this: &mut Self, player, cx| {
            this.tick(&player, cx);
        });

        info!("Discord Rich Presence initialized");

        Self {
            player: player.clone(),
            library: library.clone(),
            config: Settings::load().discord,
            sender: tx,
            last_sent_track: None,
            last_sent_time: None,
            last_sent_position: 0.0,
            _player_changed,
        }
    }

    /// Refresh settings from the active configuration and force immediate presence update.
    pub fn reload_config(&mut self, cx: &mut Context<Self>) {
        self.config = Settings::load().discord;
        info!(
            "Discord RPC settings reloaded: enabled={}, lastfm_button={}, youtube_button={}",
            self.config.enabled, self.config.show_lastfm_button, self.config.show_youtube_button
        );
        self.last_sent_track = None;
        let player = self.player.clone();
        self.tick(&player, cx);
    }

    /// React to player pump notifications on the main thread.
    fn tick(&mut self, player: &Entity<Player>, cx: &mut Context<Self>) {
        if !self.config.enabled {
            if self.last_sent_track.is_some() {
                self.last_sent_track = None;
                self.last_sent_time = None;
                info!("Discord RPC disabled; clearing presence");
                let _ = self.sender.try_send(DiscordCommand::ClearPresence);
            }
            return;
        }

        let player = player.read(cx);
        let now_playing = player.now_playing();
        let is_playing = player.is_playing();

        let current_state = now_playing.map(|now| {
            let meta = self.library.read(cx).meta_for(&now.path);
            let (title, artist, album, codec, bitrate_kbps) = match meta {
                Some(m) => (
                    if m.title.is_empty() {
                        now.path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "Unknown Track".into())
                    } else {
                        m.title
                    },
                    if m.artist.is_empty() {
                        "Unknown Artist".to_string()
                    } else {
                        m.artist
                    },
                    m.album,
                    m.codec,
                    m.bitrate_kbps,
                ),
                None => (
                    now.path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "Unknown Track".into()),
                    "Unknown Artist".to_string(),
                    String::new(),
                    now.path
                        .extension()
                        .map(|e| e.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    0,
                ),
            };

            DiscordTrackState {
                title,
                artist,
                album,
                codec,
                bitrate_kbps,
                position_secs: now.position_secs,
                duration_secs: now.duration_secs,
                is_playing,
                show_lastfm_button: self.config.show_lastfm_button,
                show_youtube_button: self.config.show_youtube_button,
            }
        });

        let now_time = SystemTime::now();

        let should_update = match (&self.last_sent_track, &current_state) {
            (None, Some(_)) => true,
            (Some(_), None) => true,
            (Some(prev), Some(curr)) => {
                if !prev.same_metadata(curr) {
                    true
                } else if curr.is_playing {
                    // Detect manual user seek (> 3s drift from elapsed clock)
                    let elapsed_real = self
                        .last_sent_time
                        .and_then(|t| now_time.duration_since(t).ok())
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(0.0);
                    let expected_pos = self.last_sent_position + elapsed_real;
                    (curr.position_secs - expected_pos).abs() > 3.0
                } else {
                    false
                }
            }
            (None, None) => false,
        };

        if should_update {
            self.last_sent_track = current_state.clone();
            self.last_sent_time = if current_state.is_some() {
                Some(now_time)
            } else {
                None
            };
            self.last_sent_position = current_state.as_ref().map(|s| s.position_secs).unwrap_or(0.0);

            let cmd = match current_state {
                Some(s) => DiscordCommand::UpdatePresence(Some(s)),
                None => DiscordCommand::ClearPresence,
            };
            let _ = self.sender.try_send(cmd);
        }
    }

    /// Background task managing socket lifecycle and activity updates.
    async fn run_ipc_loop(rx: async_channel::Receiver<DiscordCommand>) {
        let Some(app_id) = DISCORD_APP_ID else {
            return;
        };
        let mut client: Option<DiscordIpcClient> = None;
        let mut last_connect_attempt = SystemTime::UNIX_EPOCH;

        while let Ok(cmd) = rx.recv().await {
            match cmd {
                DiscordCommand::UpdatePresence(Some(state)) => {
                    // Rate-limit connect retries (5 seconds minimum backoff if client is disconnected)
                    if client.is_none() {
                        let now = SystemTime::now();
                        let time_since_last = now
                            .duration_since(last_connect_attempt)
                            .unwrap_or_default()
                            .as_secs();
                        if time_since_last >= 5 {
                            last_connect_attempt = now;
                            let mut new_client = DiscordIpcClient::new(app_id);
                            match new_client.connect() {
                                Ok(_) => {
                                    info!("Discord IPC client connected successfully");
                                    client = Some(new_client);
                                }
                                Err(e) => {
                                    warn!("Failed to connect Discord IPC client: {e}");
                                }
                            }
                        }
                    }

                    if let Some(cli) = client.as_mut() {
                        let details = state.title.clone();
                        let state_str = format!("by {}", state.artist);

                        let mut activity = Activity::new()
                            .activity_type(ActivityType::Listening)
                            .details(&details)
                            .state(&state_str);

                        // Add timestamp if playing
                        if state.is_playing {
                            let now_millis = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .map(|d| d.as_millis() as i64)
                                .unwrap_or(0);
                            let start_time =
                                now_millis.saturating_sub((state.position_secs * 1000.0) as i64);

                            let mut timestamps = Timestamps::new().start(start_time);
                            if let Some(dur) = state.duration_secs {
                                let end_time = start_time + ((dur * 1000.0) as i64);
                                timestamps = timestamps.end(end_time);
                            }
                            activity = activity.timestamps(timestamps);
                        }

                        // Attempt to resolve cover art URL online via iTunes / Deezer / Last.fm providers
                        let mut cover_url: Option<String> = None;
                        if !state.artist.is_empty() || !state.album.is_empty() {
                            let query = crate::providers::TrackQuery {
                                artist: state.artist.clone(),
                                album: state.album.clone(),
                                title: state.title.clone(),
                                duration_secs: state.duration_secs,
                            };
                            match crate::providers::search_art(&query) {
                                Ok(candidates) => {
                                    // Prioritize Deezer > Last.fm > iTunes for presence cover art,
                                    // iTunes sometimes returns cover art for multiple albums 
                                    // by the creator which makes us use the wrong art?
                                    let chosen = candidates
                                        .iter()
                                        .find(|c| c.provider.eq_ignore_ascii_case("deezer"))
                                        .or_else(|| {
                                            candidates
                                                .iter()
                                                .find(|c| c.provider.eq_ignore_ascii_case("lastfm"))
                                        })
                                        .or_else(|| {
                                            candidates
                                                .iter()
                                                .find(|c| c.provider.eq_ignore_ascii_case("itunes"))
                                        })
                                        .or_else(|| candidates.first());

                                    if let Some(c) = chosen {
                                        cover_url = Some(c.full_url.clone());
                                        info!(
                                            "Resolved cover art for '{}' via {}: {}",
                                            state.title, c.provider, c.full_url
                                        );
                                    }
                                }
                                Err(e) => {
                                    warn!("Artwork search error for '{}': {e}", state.title);
                                }
                            }
                        }

                        let image_key = cover_url.as_deref().unwrap_or("app_icon");
                        let format_str = format_quality(&state.codec, state.bitrate_kbps);

                        let large_text = match (!state.album.is_empty(), !format_str.is_empty()) {
                            (true, true) => format!("{} • {}", state.album, format_str),
                            (true, false) => state.album.clone(),
                            (false, true) => format_str,
                            (false, false) => state.title.clone(),
                        };

                        let (small_key, small_text) = if state.is_playing {
                            ("play", "Playing")
                        } else {
                            ("pause", "Paused")
                        };

                        let assets = Assets::new()
                            .large_image(image_key)
                            .large_text(&large_text)
                            .small_image(small_key)
                            .small_text(small_text);
                        activity = activity.assets(assets);

                        // Add clickable buttons if enabled and artist/title available
                        let mut buttons = Vec::new();
                        let lastfm_url = format!(
                            "https://www.last.fm/music/{}/_/{}",
                            url_encode(&state.artist),
                            url_encode(&state.title)
                        );
                        let youtube_url = format!(
                            "https://www.youtube.com/results?search_query={}+{}",
                            url_encode(&state.artist),
                            url_encode(&state.title)
                        );
                        let has_meta = !state.artist.is_empty() || !state.title.is_empty();

                        if state.show_lastfm_button && has_meta {
                            buttons.push(Button::new("View on Last.fm", &lastfm_url));
                        }
                        if state.show_youtube_button && has_meta {
                            buttons.push(Button::new("Search on YouTube", &youtube_url));
                        }

                        if !buttons.is_empty() {
                            activity = activity.buttons(buttons);
                        }

                        if let Err(e) = cli.set_activity(activity) {
                            error!("Discord RPC set_activity failed: {e}");
                            let _ = cli.close();
                            client = None;
                        } else {
                            info!("Discord RPC status updated: '{}' by '{}'", state.title, state.artist);
                        }
                    }
                }
                DiscordCommand::UpdatePresence(None) | DiscordCommand::ClearPresence => {
                    if let Some(cli) = client.as_mut() {
                        let _ = cli.clear_activity();
                        info!("Discord RPC status cleared");
                    }
                }
            }
        }

        if let Some(mut cli) = client {
            let _ = cli.close();
            info!("Discord IPC loop closed");
        }
    }
}

/// Standard UTF-8 percent-encoding for URLs.
fn url_encode(input: &str) -> String {
    url::form_urlencoded::byte_serialize(input.as_bytes()).collect()
}

/// Format audio quality text based on codec and bitrate.
fn format_quality(codec: &str, bitrate_kbps: u16) -> String {
    if codec.is_empty() {
        return String::new();
    }
    let codec_upper = codec.to_uppercase();
    match codec_upper.as_str() {
        "FLAC" | "ALAC" | "WAV" | "AIFF" | "PCM" => {
            format!("{codec_upper} Lossless")
        }
        "MP3" | "AAC" | "OGG" | "OPUS" | "M4A" | "VORBIS" => {
            if bitrate_kbps > 0 {
                format!("{codec_upper} {bitrate_kbps} kbps")
            } else {
                format!("{codec_upper} VBR")
            }
        }
        _ => {
            if bitrate_kbps > 0 {
                format!("{codec_upper} {bitrate_kbps} kbps")
            } else {
                codec_upper
            }
        }
    }
}
