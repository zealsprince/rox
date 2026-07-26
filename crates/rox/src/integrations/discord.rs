//! Discord Rich Presence integration: publishes the now-playing track,
//! playback status (playing/paused), and elapsed timestamps over Discord IPC.
//!
//! Socket communication and reconnects run on a background thread to prevent
//! stalling the main GPUI thread or audio path.

use std::time::{SystemTime, UNIX_EPOCH};

use discord_rich_presence::activity::{Activity, ActivityType, Assets, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use gpui::{Context, Entity, Subscription};

use crate::panels::library::Library;
use crate::player::Player;
use crate::settings::{DiscordSettings, Settings};

/// Default Discord Client Application ID for rox.
const DISCORD_APP_ID: &str = "1530943456543772732";

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
    pub position_secs: f64,
    pub duration_secs: Option<f64>,
    pub is_playing: bool,
}

pub struct DiscordPresence {
    library: Entity<Library>,
    config: DiscordSettings,
    sender: async_channel::Sender<DiscordCommand>,
    last_state: Option<DiscordTrackState>,
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

        Self {
            library: library.clone(),
            config: Settings::load().discord,
            sender: tx,
            last_state: None,
            _player_changed,
        }
    }

    /// Refresh settings from the active configuration.
    pub fn reload_config(&mut self) {
        self.config = Settings::load().discord;
    }

    /// React to player pump notifications on the main thread.
    fn tick(&mut self, player: &Entity<Player>, cx: &mut Context<Self>) {
        if !self.config.enabled {
            if self.last_state.is_some() {
                self.last_state = None;
                let _ = self.sender.try_send(DiscordCommand::ClearPresence);
            }
            return;
        }

        let player = player.read(cx);
        let now_playing = player.now_playing();
        let is_playing = player.is_playing();

        let current_state = now_playing.and_then(|now| {
            let meta = self.library.read(cx).meta_for(&now.path);
            let (title, artist, album) = match meta {
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
                ),
                None => (
                    now.path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| "Unknown Track".into()),
                    "Unknown Artist".to_string(),
                    String::new(),
                ),
            };

            Some(DiscordTrackState {
                title,
                artist,
                album,
                position_secs: now.position_secs,
                duration_secs: now.duration_secs,
                is_playing,
            })
        });

        // State gating: avoid redundant updates if track state hasn't moved
        if self.last_state != current_state {
            self.last_state = current_state.clone();
            let cmd = match current_state {
                Some(s) => DiscordCommand::UpdatePresence(Some(s)),
                None => DiscordCommand::ClearPresence,
            };
            let _ = self.sender.try_send(cmd);
        }
    }

    /// Background task managing socket lifecycle and activity updates.
    async fn run_ipc_loop(rx: async_channel::Receiver<DiscordCommand>) {
        let mut client: Option<DiscordIpcClient> = None;

        while let Ok(cmd) = rx.recv().await {
            match cmd {
                DiscordCommand::UpdatePresence(Some(state)) => {
                    // Ensure connected client
                    if client.is_none() {
                        let mut new_client = DiscordIpcClient::new(DISCORD_APP_ID);
                        if new_client.connect().is_ok() {
                            client = Some(new_client);
                        }
                    }

                    if let Some(cli) = client.as_mut() {
                        let details = state.title.clone();
                        let state_str = format!("by {}", state.artist);

                        let mut activity = Activity::new()
                            .activity_type(ActivityType::Listening)
                            .details(&details)
                            .state(&state_str);

                        // Add timestamp if enabled and playing
                        if state.is_playing {
                            let now_millis = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .map(|d| d.as_millis() as i64)
                                .unwrap_or(0);
                            let start_time = now_millis.saturating_sub((state.position_secs * 1000.0) as i64);

                            let mut timestamps = Timestamps::new().start(start_time);
                            if let Some(dur) = state.duration_secs {
                                let end_time = start_time + ((dur * 1000.0) as i64);
                                timestamps = timestamps.end(end_time);
                            }
                            activity = activity.timestamps(timestamps);
                        }

                        // Attempt to resolve cover art URL online via iTunes / Deezer providers
                        let mut cover_url: Option<String> = None;
                        if !state.artist.is_empty() || !state.album.is_empty() {
                            let query = crate::providers::TrackQuery {
                                artist: state.artist.clone(),
                                album: state.album.clone(),
                                title: state.title.clone(),
                                duration_secs: state.duration_secs,
                            };
                            if let Ok(candidates) = crate::providers::search_art(&query) {
                                if let Some(first) = candidates.first() {
                                    cover_url = Some(first.thumb_url.clone());
                                }
                            }
                        }

                        let image_key = cover_url.as_deref().unwrap_or("app_icon");
                        let large_text = if state.album.is_empty() {
                            state.title.as_str()
                        } else {
                            state.album.as_str()
                        };

                        let assets = Assets::new()
                            .large_image(image_key)
                            .large_text(large_text);
                        activity = activity.assets(assets);

                        if let Err(e) = cli.set_activity(activity) {
                            log::warn!("discord rpc set_activity failed: {e}");
                            // Reconnect on failure next time
                            let _ = cli.close();
                            client = None;
                        }
                    }
                }
                DiscordCommand::UpdatePresence(None) | DiscordCommand::ClearPresence => {
                    if let Some(cli) = client.as_mut() {
                        let _ = cli.clear_activity();
                    }
                }
            }
        }

        if let Some(mut cli) = client {
            let _ = cli.close();
        }
    }
}
