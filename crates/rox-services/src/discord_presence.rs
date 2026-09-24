//! Discord Rich Presence: the now-playing card over Discord IPC. The socket
//! and its reconnects run on a background task, off the UI thread.

use std::time::{SystemTime, UNIX_EPOCH};

use discord_rich_presence::activity::{
    Activity, ActivityType, Assets, Button, StatusDisplayType, Timestamps,
};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use gpui::{Context, Entity, Subscription};
use log::{error, info, warn};

use rox_core::pattern::{self, Name, Pattern, PatternField};
use rox_core::settings::{
    DEFAULT_PRESENCE_FIRST_LINE, DEFAULT_PRESENCE_HOVER, DEFAULT_PRESENCE_SECOND_LINE,
    DiscordSettings, DiscordStatusLine, Settings,
};

use crate::catalog::Library;
use crate::player::Player;

/// Clears a reissue suffix or a lightly renamed artist, not a different
/// album by the same artist.
const ART_MATCH_BAR: f32 = 0.5;

/// Every placeholder in the app-wide vocabulary parses; what the track
/// can't answer renders as nothing.
#[derive(Clone, PartialEq)]
pub enum PresenceField {
    Artist,
    AlbumArtist,
    Album,
    Title,
    Track,
    Year,
    Genre,
    Station,
    Source,
    Format,
    /// A placeholder the card can't fill: it renders nothing rather than
    /// refusing a pattern the rename dialog would accept.
    Unfilled,
}

impl PatternField for PresenceField {
    fn from_name(name: Name) -> Option<Self> {
        Some(match name {
            Name::Artist => PresenceField::Artist,
            Name::AlbumArtist => PresenceField::AlbumArtist,
            Name::Album => PresenceField::Album,
            Name::Title => PresenceField::Title,
            Name::Track => PresenceField::Track,
            Name::Year | Name::Date => PresenceField::Year,
            Name::Genre => PresenceField::Genre,
            Name::Station => PresenceField::Station,
            Name::Source => PresenceField::Source,
            Name::Format => PresenceField::Format,
            Name::Comment | Name::Disc => PresenceField::Unfilled,
            Name::Skip => return None,
        })
    }

    /// Every field may vanish, unlike the renamer's: "Unknown Album" on every
    /// untagged track is worse than no line.
    fn fallback(&self) -> &'static str {
        ""
    }
}

pub struct PresenceSample {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub genre: String,
    /// Empty when the track carries none, so the pattern closes its hole.
    pub year: String,
    pub track: String,
    pub station: String,
    pub source: String,
    pub codec: String,
    pub bitrate_kbps: u16,
}

/// The fully tagged stand-in the settings rows preview against.
impl Default for PresenceSample {
    fn default() -> Self {
        PresenceSample {
            title: "Xtal".into(),
            artist: "Aphex Twin".into(),
            album: "Selected Ambient Works 85-92".into(),
            album_artist: "Aphex Twin".into(),
            genre: "Electronic".into(),
            year: "1992".into(),
            track: "1".into(),
            station: "Noise FM".into(),
            source: "Library".into(),
            codec: "FLAC".into(),
            bitrate_kbps: 936,
        }
    }
}

impl PresenceSample {
    pub fn playing(player: &Player, library: &Library) -> Option<Self> {
        let now = player.now_playing()?;
        // Through the player, so a station shows the song it announced.
        let meta = player.live_over(library.meta_for_key(&now.key));

        let file_name = || {
            now.path()
                .and_then(|path| path.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Unknown Track".into())
        };
        let number = |n: u16| if n == 0 { String::new() } else { n.to_string() };
        let station = player
            .station_info()
            .map(|info| info.name)
            .unwrap_or_default();
        let source = crate::capture::source_label(&now.key.source);

        let Some(meta) = meta else {
            return Some(PresenceSample {
                title: file_name(),
                artist: "Unknown Artist".into(),
                album: String::new(),
                album_artist: String::new(),
                genre: String::new(),
                year: String::new(),
                track: String::new(),
                station,
                source,
                codec: now
                    .path()
                    .and_then(|path| path.extension())
                    .map(|e| e.to_string_lossy().to_string())
                    .unwrap_or_default(),
                bitrate_kbps: 0,
            });
        };

        Some(PresenceSample {
            title: if meta.title.is_empty() {
                file_name()
            } else {
                meta.title
            },
            artist: if meta.artist.is_empty() {
                "Unknown Artist".into()
            } else {
                meta.artist
            },
            album: meta.album,
            album_artist: meta.album_artist,
            genre: meta.genre,
            year: number(meta.year),
            track: number(meta.track_no),
            station,
            source,
            codec: meta.codec,
            bitrate_kbps: meta.bitrate_kbps,
        })
    }

    fn values(&self) -> Vec<(PresenceField, String)> {
        vec![
            (PresenceField::Artist, self.artist.clone()),
            (PresenceField::AlbumArtist, self.album_artist.clone()),
            (PresenceField::Album, self.album.clone()),
            (PresenceField::Title, self.title.clone()),
            (PresenceField::Track, self.track.clone()),
            (PresenceField::Year, self.year.clone()),
            (PresenceField::Genre, self.genre.clone()),
            (PresenceField::Station, self.station.clone()),
            (PresenceField::Source, self.source.clone()),
            (
                PresenceField::Format,
                format_quality(&self.codec, self.bitrate_kbps),
            ),
            (PresenceField::Unfilled, String::new()),
        ]
    }
}

/// Runs the same parse and render an update takes, so the settings preview
/// can't drift from the card.
pub fn preview(text: &str, sample: &PresenceSample) -> Result<String, String> {
    Ok(pattern::parse_line::<PresenceField>(text)?.render_line(&sample.values()))
}

/// A pattern that no longer parses falls back to the default rather than
/// blanking the card. Blank text parses fine and turns the line off.
fn line(text: &str, default: &str) -> Pattern<PresenceField> {
    match pattern::parse_line(text) {
        Ok(parsed) => parsed,
        Err(e) => {
            warn!("discord: {text:?} is not a pattern ({e}), using the default");
            pattern::parse_line(default).expect("the default presence line parses")
        }
    }
}

pub enum DiscordCommand {
    UpdatePresence(Option<DiscordTrackState>),
    ClearPresence,
}

/// The card lines arrive rendered: the worker only publishes text.
#[derive(Clone, Debug, PartialEq)]
pub struct DiscordTrackState {
    pub first_line: String,
    pub second_line: String,
    pub hover_line: String,
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
    pub status_line: DiscordStatusLine,
}

impl DiscordTrackState {
    /// Everything but `position_secs`, which moves continuously.
    pub fn same_metadata(&self, other: &Self) -> bool {
        self.first_line == other.first_line
            && self.second_line == other.second_line
            && self.hover_line == other.hover_line
            && self.title == other.title
            && self.artist == other.artist
            && self.album == other.album
            && self.codec == other.codec
            && self.bitrate_kbps == other.bitrate_kbps
            && self.duration_secs == other.duration_secs
            && self.is_playing == other.is_playing
            && self.show_lastfm_button == other.show_lastfm_button
            && self.show_youtube_button == other.show_youtube_button
            && self.status_line == other.status_line
    }
}

pub struct DiscordPresence {
    player: Entity<Player>,
    library: Entity<Library>,
    config: DiscordSettings,
    first_line: Pattern<PresenceField>,
    second_line: Pattern<PresenceField>,
    hover_line: Pattern<PresenceField>,
    /// Kept for the settings preview, so it needs no library lookup in a
    /// render pass.
    last_sample: Option<PresenceSample>,
    sender: async_channel::Sender<DiscordCommand>,
    last_sent_track: Option<DiscordTrackState>,
    last_sent_time: Option<SystemTime>,
    last_sent_position: f64,
    _player_changed: Subscription,
}

impl DiscordPresence {
    pub fn new(player: &Entity<Player>, library: &Entity<Library>, cx: &mut Context<Self>) -> Self {
        let (tx, rx) = async_channel::bounded::<DiscordCommand>(16);

        cx.background_executor()
            .spawn(async move {
                Self::run_ipc_loop(rx).await;
            })
            .detach();

        let _player_changed = cx.observe(player, |this: &mut Self, player, cx| {
            this.tick(&player, cx);
        });

        info!("Discord Rich Presence initialized");

        let config = Settings::load().accounts.discord;

        Self {
            player: player.clone(),
            library: library.clone(),
            first_line: line(&config.first_line, DEFAULT_PRESENCE_FIRST_LINE),
            second_line: line(&config.second_line, DEFAULT_PRESENCE_SECOND_LINE),
            hover_line: line(&config.hover_line, DEFAULT_PRESENCE_HOVER),
            config,
            sender: tx,
            last_sample: None,
            last_sent_track: None,
            last_sent_time: None,
            last_sent_position: 0.0,
            _player_changed,
        }
    }

    pub fn reload_config(&mut self, cx: &mut Context<Self>) {
        self.config = Settings::load().accounts.discord;
        self.first_line = line(&self.config.first_line, DEFAULT_PRESENCE_FIRST_LINE);
        self.second_line = line(&self.config.second_line, DEFAULT_PRESENCE_SECOND_LINE);
        self.hover_line = line(&self.config.hover_line, DEFAULT_PRESENCE_HOVER);
        info!(
            "Discord RPC settings reloaded: enabled={}, lastfm_button={}, youtube_button={}, status_line={:?}, lines={:?} / {:?}, hover={:?}",
            self.config.enabled,
            self.config.show_lastfm_button,
            self.config.show_youtube_button,
            self.config.status_line,
            self.config.first_line,
            self.config.second_line,
            self.config.hover_line
        );
        self.last_sent_track = None;
        let player = self.player.clone();
        self.tick(&player, cx);
    }

    /// Against the playing track, or the stand-in with nothing on.
    pub fn preview_line(&self, text: &str) -> Result<String, String> {
        match self.last_sample.as_ref() {
            Some(sample) => preview(text, sample),
            None => preview(text, &PresenceSample::default()),
        }
    }

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
        let sample = PresenceSample::playing(player, self.library.read(cx));

        let current_state = match (now_playing, &sample) {
            (Some(now), Some(sample)) => {
                let values = sample.values();

                Some(DiscordTrackState {
                    first_line: self.first_line.render_line(&values),
                    second_line: self.second_line.render_line(&values),
                    hover_line: self.hover_line.render_line(&values),
                    // Raw tags beside the rendered lines: the art search
                    // and the buttons want them unformatted.
                    title: sample.title.clone(),
                    artist: sample.artist.clone(),
                    album: sample.album.clone(),
                    codec: sample.codec.clone(),
                    bitrate_kbps: sample.bitrate_kbps,
                    position_secs: now.position_secs,
                    duration_secs: now.duration_secs,
                    is_playing,
                    show_lastfm_button: self.config.show_lastfm_button,
                    show_youtube_button: self.config.show_youtube_button,
                    status_line: self.config.status_line,
                })
            }
            _ => None,
        };

        self.last_sample = sample;

        let now_time = SystemTime::now();

        let should_update = match (&self.last_sent_track, &current_state) {
            (None, Some(_)) => true,
            (Some(_), None) => true,
            (Some(prev), Some(curr)) => {
                if !prev.same_metadata(curr) {
                    true
                } else if curr.is_playing {
                    // A manual seek shows as more than 3s drift from the clock.
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
            self.last_sent_position = current_state
                .as_ref()
                .map(|s| s.position_secs)
                .unwrap_or(0.0);

            let cmd = match current_state {
                Some(s) => DiscordCommand::UpdatePresence(Some(s)),
                None => DiscordCommand::ClearPresence,
            };
            let _ = self.sender.try_send(cmd);
        }
    }

    async fn run_ipc_loop(rx: async_channel::Receiver<DiscordCommand>) {
        if !rox_net::discord::has_builtin_application_id() {
            info!("Discord presence disabled: no application id baked into this build");
            return;
        }
        let app_id = rox_net::discord::keys::APPLICATION_ID;
        let mut client: Option<DiscordIpcClient> = None;
        let mut last_connect_attempt = SystemTime::UNIX_EPOCH;

        while let Ok(cmd) = rx.recv().await {
            match cmd {
                DiscordCommand::UpdatePresence(Some(state)) => {
                    // At most one connect attempt every 5 seconds.
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
                        let status_display = match state.status_line {
                            DiscordStatusLine::App => StatusDisplayType::Name,
                            DiscordStatusLine::First => StatusDisplayType::Details,
                            DiscordStatusLine::Second => StatusDisplayType::State,
                        };

                        let mut activity = Activity::new()
                            .activity_type(ActivityType::Listening)
                            .status_display_type(status_display);

                        // Leave an empty line off: an empty string is a
                        // blank row on the card.
                        if !state.first_line.is_empty() {
                            activity = activity.details(&state.first_line);
                        }
                        if !state.second_line.is_empty() {
                            activity = activity.state(&state.second_line);
                        }

                        // Timestamps only while playing: Discord counts on
                        // from the start stamp client-side, so a paused track
                        // would keep counting. Resume re-anchors, since
                        // same_metadata counts is_playing.
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

                        let mut cover_url: Option<String> = None;
                        if !state.artist.is_empty() || !state.album.is_empty() {
                            let query = rox_net::providers::TrackQuery {
                                artist: state.artist.clone(),
                                album: state.album.clone(),
                                title: state.title.clone(),
                                duration_secs: state.duration_secs,
                            };
                            match rox_net::providers::search_art(&query) {
                                Ok(candidates) => {
                                    // Provider order can put a different album's cover
                                    // first (issue #79), so score every candidate and
                                    // take the closest; provider rank only breaks ties.
                                    let rank = |provider: &str| match provider {
                                        "deezer" => 0,
                                        "lastfm" => 1,
                                        "itunes" => 2,
                                        _ => 3,
                                    };
                                    let chosen = candidates
                                        .iter()
                                        .map(|c| (c, rox_net::providers::art_confidence(&query, c)))
                                        .filter(|(_, score)| *score >= ART_MATCH_BAR)
                                        .min_by(|(a, sa), (b, sb)| {
                                            sb.partial_cmp(sa)
                                                .unwrap_or(std::cmp::Ordering::Equal)
                                                .then_with(|| {
                                                    rank(a.provider).cmp(&rank(b.provider))
                                                })
                                        })
                                        .map(|(c, _)| c);

                                    if let Some(c) = chosen {
                                        cover_url = Some(c.full_url.clone());
                                        info!(
                                            "Resolved cover art for '{}' via {}: {}",
                                            state.title, c.provider, c.full_url
                                        );
                                    } else if !candidates.is_empty() {
                                        info!(
                                            "No cover art candidate matched '{}' closely enough; using app icon",
                                            state.title
                                        );
                                    }
                                }
                                Err(e) => {
                                    warn!("Artwork search error for '{}': {e}", state.title);
                                }
                            }
                        }

                        let image_key = cover_url.as_deref().unwrap_or("app_icon");

                        let (small_key, small_text) = if state.is_playing {
                            ("play", "Playing")
                        } else {
                            ("pause", "Paused")
                        };

                        let mut assets = Assets::new()
                            .large_image(image_key)
                            .small_image(small_key)
                            .small_text(small_text);

                        // No hover text rather than an empty one, which
                        // Discord draws as a blank tooltip.
                        if !state.hover_line.is_empty() {
                            assets = assets.large_text(&state.hover_line);
                        }

                        activity = activity.assets(assets);

                        let mut buttons = Vec::new();
                        let lastfm_url = format!(
                            "https://www.Last.fm/music/{}/_/{}",
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
                            info!(
                                "Discord RPC status updated: '{}' / '{}'",
                                state.first_line, state.second_line
                            );
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

fn url_encode(input: &str) -> String {
    url::form_urlencoded::byte_serialize(input.as_bytes()).collect()
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_lines_name_the_track_and_its_album() {
        let sample = PresenceSample::default();

        assert_eq!(
            preview(DEFAULT_PRESENCE_FIRST_LINE, &sample).unwrap(),
            "Aphex Twin - Xtal"
        );
        assert_eq!(
            preview(DEFAULT_PRESENCE_SECOND_LINE, &sample).unwrap(),
            "Selected Ambient Works 85-92"
        );
    }

    /// Stations often send a title and nothing else, and " - " under the
    /// name is worse than one line.
    #[test]
    fn a_missing_tag_closes_its_own_hole() {
        let bare = PresenceSample {
            album: String::new(),
            year: String::new(),
            ..PresenceSample::default()
        };

        assert_eq!(preview("%artist% - %album%", &bare).unwrap(), "Aphex Twin");
        assert_eq!(preview("%album% (%year%)", &bare).unwrap(), "");
    }

    #[test]
    fn the_hover_text_ships_as_the_quality() {
        let sample = PresenceSample::default();

        assert_eq!(
            preview(DEFAULT_PRESENCE_HOVER, &sample).unwrap(),
            "FLAC Lossless"
        );
        assert_eq!(
            preview("%album% • %format%", &sample).unwrap(),
            "Selected Ambient Works 85-92 • FLAC Lossless"
        );
    }

    #[test]
    fn a_line_can_name_the_station_and_the_source() {
        let sample = PresenceSample::default();

        assert_eq!(
            preview("%station% (%source%)", &sample).unwrap(),
            "Noise FM (Library)"
        );
        let file = PresenceSample {
            station: String::new(),
            ..PresenceSample::default()
        };
        assert_eq!(preview("%station% - %title%", &file).unwrap(), "Xtal");
    }

    #[test]
    fn a_typoed_placeholder_is_refused_and_falls_back() {
        assert!(preview("%tittle%", &PresenceSample::default()).is_err());
        assert_eq!(
            line("%tittle%", DEFAULT_PRESENCE_FIRST_LINE)
                .render_line(&PresenceSample::default().values()),
            "Aphex Twin - Xtal"
        );
    }

    #[test]
    fn a_blank_line_renders_nothing() {
        assert_eq!(preview("", &PresenceSample::default()).unwrap(), "");
    }
}
