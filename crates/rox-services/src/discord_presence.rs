//! Discord Rich Presence integration: publishes the now-playing track,
//! playback status (playing/paused), and elapsed timestamps over Discord IPC.
//!
//! Socket communication and reconnects run on a background thread to prevent
//! stalling the main GPUI thread or audio path.

use std::time::{SystemTime, UNIX_EPOCH};

use discord_rich_presence::activity::{
    Activity, ActivityType, Assets, Button, StatusDisplayType, Timestamps,
};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient};
use gpui::{Context, Entity, Subscription};
use log::{error, info, warn};

use rox_core::pattern::{self, Pattern, PatternField};
use rox_core::settings::{
    DEFAULT_PRESENCE_FIRST_LINE, DEFAULT_PRESENCE_SECOND_LINE, DiscordSettings, DiscordStatusLine,
    Settings,
};

use crate::catalog::Library;
use crate::player::Player;

/// The score below which no online cover is close enough to show. A right
/// album with a reissue suffix or a lightly renamed artist clears it; a
/// different album by the same artist does not.
const ART_MATCH_BAR: f32 = 0.5;

/// The placeholders a card line can fill, the help line's source of
/// truth. The renamer's names minus the tags a playing track has nothing
/// for, plus the format the card used to keep in hover text alone.
pub const PRESENCE_PLACEHOLDERS: &[&str] = &[
    "%artist%",
    "%albumartist%",
    "%album%",
    "%title%",
    "%track%",
    "%year%",
    "%genre%",
    "%format%",
];

/// What a card line may name. The renamer's vocabulary wherever a playing
/// track can fill it, and the tags it can't parse and render nothing
/// rather than refusing a pattern the rename dialog would have accepted.
#[derive(Clone, PartialEq)]
pub enum PresenceField {
    Artist,
    AlbumArtist,
    Album,
    Title,
    Track,
    Year,
    Genre,
    /// The codec and bitrate as one phrase, "FLAC Lossless".
    Format,
    /// A placeholder the card can't fill: a comment, a disc number. It
    /// parses and renders nothing, taking its separator with it.
    Unfilled,
}

impl PatternField for PresenceField {
    fn from_placeholder(name: &str) -> Result<Option<Self>, String> {
        Ok(Some(match name {
            "artist" => PresenceField::Artist,
            "albumartist" | "album artist" => PresenceField::AlbumArtist,
            "album" => PresenceField::Album,
            "title" => PresenceField::Title,
            "track" | "tracknumber" => PresenceField::Track,
            // The renamer reads both as the release year, and a card has
            // no other date to mean.
            "year" | "date" => PresenceField::Year,
            "genre" => PresenceField::Genre,
            "format" => PresenceField::Format,
            "comment" | "disc" | "discnumber" => PresenceField::Unfilled,
            "skip" | "dummy" | "ignore" => return Ok(None),
            other => {
                return Err(rox_i18n::t!(
                    "tags-guess-unknown-placeholder",
                    name = other.to_owned()
                )
                .to_string());
            }
        }))
    }

    /// Every field here is allowed to vanish, which is the opposite of
    /// the renamer's rule: a file still needs a name, and a card line is
    /// read at a glance, where "Unknown Album" on every untagged track is
    /// worse than the line not being there at all.
    fn fallback(&self) -> &'static str {
        ""
    }
}

/// The values a card line renders against: what's playing, with the
/// fills the card has always shown for a track nobody has tagged.
pub struct PresenceSample {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub genre: String,
    /// Both numbers as text, empty when the track carries neither, so a
    /// pattern that names one closes its own hole.
    pub year: String,
    pub track: String,
    pub codec: String,
    pub bitrate_kbps: u16,
}

/// The stand-in the settings rows preview against with nothing playing,
/// in the capture row's register: a real record, fully tagged, so every
/// placeholder shows what it does.
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
            codec: "FLAC".into(),
            bitrate_kbps: 936,
        }
    }
}

impl PresenceSample {
    /// What's playing, or None with nothing on.
    pub fn playing(player: &Player, library: &Library) -> Option<Self> {
        let now = player.now_playing()?;
        // Through the player's accessor, so a station's presence shows
        // the song it just announced rather than the station row's own
        // title for the whole evening.
        let meta = player.live_over(library.meta_for_key(&now.key));

        // Untagged and unknown to the library: the file name is the only
        // name there is, and a remote track doesn't even have that, so it
        // shows as unknown until the source fills it in.
        let file_name = || {
            now.path()
                .and_then(|path| path.file_name())
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Unknown Track".into())
        };
        let number = |n: u16| if n == 0 { String::new() } else { n.to_string() };

        let Some(meta) = meta else {
            return Some(PresenceSample {
                title: file_name(),
                artist: "Unknown Artist".into(),
                album: String::new(),
                album_artist: String::new(),
                genre: String::new(),
                year: String::new(),
                track: String::new(),
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
            codec: meta.codec,
            bitrate_kbps: meta.bitrate_kbps,
        })
    }

    /// The sample as a pattern's values.
    fn values(&self) -> Vec<(PresenceField, String)> {
        vec![
            (PresenceField::Artist, self.artist.clone()),
            (PresenceField::AlbumArtist, self.album_artist.clone()),
            (PresenceField::Album, self.album.clone()),
            (PresenceField::Title, self.title.clone()),
            (PresenceField::Track, self.track.clone()),
            (PresenceField::Year, self.year.clone()),
            (PresenceField::Genre, self.genre.clone()),
            (
                PresenceField::Format,
                format_quality(&self.codec, self.bitrate_kbps),
            ),
            (PresenceField::Unfilled, String::new()),
        ]
    }
}

/// What `text` would put on the card, or what's wrong with it. Runs the
/// same parse and render an update takes, so the line under the settings
/// input can't drift from the line Discord shows.
pub fn preview(text: &str, sample: &PresenceSample) -> Result<String, String> {
    Ok(pattern::parse_line::<PresenceField>(text)?.render_line(&sample.values()))
}

/// The pattern a line is configured with, or the default when what's in
/// settings no longer parses. A hand-edited file or a placeholder retired
/// from under it can't be allowed to blank the card, and the default is
/// always right. Blank text is not a failure: it parses to a pattern that
/// renders nothing, which is how a line is turned off.
fn line(text: &str, default: &str) -> Pattern<PresenceField> {
    match pattern::parse_line(text) {
        Ok(parsed) => parsed,
        Err(e) => {
            warn!("discord: {text:?} is not a pattern ({e}), using the default");
            pattern::parse_line(default).expect("the default presence line parses")
        }
    }
}

/// Commands sent from the GPUI main thread to the background IPC worker loop.
pub enum DiscordCommand {
    UpdatePresence(Option<DiscordTrackState>),
    ClearPresence,
}

/// Snapshot of the currently playing track state sent over channel.
/// The two card lines arrive rendered: the patterns are the main
/// thread's, and the worker publishes text.
#[derive(Clone, Debug, PartialEq)]
pub struct DiscordTrackState {
    pub first_line: String,
    pub second_line: String,
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
    /// Compare all track metadata fields except position_secs (which updates continuously).
    pub fn same_metadata(&self, other: &Self) -> bool {
        self.first_line == other.first_line
            && self.second_line == other.second_line
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
    /// The card's two lines, parsed when the config is read so a tick
    /// renders rather than re-parses.
    first_line: Pattern<PresenceField>,
    second_line: Pattern<PresenceField>,
    /// What the last tick read off the playing track, kept for the
    /// settings rows' preview. Reading it again from there would put a
    /// library lookup in a render pass, and the values are the same ones.
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

        let config = Settings::load().accounts.discord;

        Self {
            player: player.clone(),
            library: library.clone(),
            first_line: line(&config.first_line, DEFAULT_PRESENCE_FIRST_LINE),
            second_line: line(&config.second_line, DEFAULT_PRESENCE_SECOND_LINE),
            config,
            sender: tx,
            last_sample: None,
            last_sent_track: None,
            last_sent_time: None,
            last_sent_position: 0.0,
            _player_changed,
        }
    }

    /// Refresh settings from the active configuration and force immediate presence update.
    pub fn reload_config(&mut self, cx: &mut Context<Self>) {
        self.config = Settings::load().accounts.discord;
        self.first_line = line(&self.config.first_line, DEFAULT_PRESENCE_FIRST_LINE);
        self.second_line = line(&self.config.second_line, DEFAULT_PRESENCE_SECOND_LINE);
        info!(
            "Discord RPC settings reloaded: enabled={}, lastfm_button={}, youtube_button={}, status_line={:?}, lines={:?} / {:?}",
            self.config.enabled,
            self.config.show_lastfm_button,
            self.config.show_youtube_button,
            self.config.status_line,
            self.config.first_line,
            self.config.second_line
        );
        self.last_sent_track = None;
        let player = self.player.clone();
        self.tick(&player, cx);
    }

    /// What `text` would put on the card, for the settings rows that type
    /// the two lines. Against the playing track when there is one and the
    /// stand-in otherwise, so a line can be dialed in with nothing on.
    pub fn preview_line(&self, text: &str) -> Result<String, String> {
        match self.last_sample.as_ref() {
            Some(sample) => preview(text, sample),
            None => preview(text, &PresenceSample::default()),
        }
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
        let sample = PresenceSample::playing(player, self.library.read(cx));

        // Both are Some or both None: the sample is read off the same
        // now-playing row.
        let current_state = match (now_playing, &sample) {
            (Some(now), Some(sample)) => {
                let values = sample.values();

                Some(DiscordTrackState {
                    first_line: self.first_line.render_line(&values),
                    second_line: self.second_line.render_line(&values),
                    // The raw tags ride along beside the rendered lines:
                    // the cover art search and the two buttons want the
                    // artist and title as tags rather than as whatever
                    // the pattern made of them.
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

    /// Background task managing socket lifecycle and activity updates.
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
                        let status_display = match state.status_line {
                            DiscordStatusLine::App => StatusDisplayType::Name,
                            DiscordStatusLine::First => StatusDisplayType::Details,
                            DiscordStatusLine::Second => StatusDisplayType::State,
                        };

                        let mut activity = Activity::new()
                            .activity_type(ActivityType::Listening)
                            .status_display_type(status_display);

                        // A line that rendered to nothing is left off the
                        // card rather than sent empty: an empty string is a
                        // blank row on the card, and no field at all closes
                        // the gap.
                        if !state.first_line.is_empty() {
                            activity = activity.details(&state.first_line);
                        }
                        if !state.second_line.is_empty() {
                            activity = activity.state(&state.second_line);
                        }

                        // Timestamps only while playing. A start stamp is an anchor, not a
                        // clock Discord ever stops: it counts on from there client-side and
                        // nothing goes out while paused, so a track left paused for twenty
                        // minutes would read 21:00 into a four-minute song. Off means no
                        // running counter at all, which is accurate for a pause.
                        // Discord draws the progress bar from the end stamp, so that
                        // goes out too.
                        // Resume re-anchors on the position we're actually at, since
                        // same_metadata counts is_playing and both transitions re-send.
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
                            let query = rox_net::providers::TrackQuery {
                                artist: state.artist.clone(),
                                album: state.album.clone(),
                                title: state.title.clone(),
                                duration_secs: state.duration_secs,
                            };
                            match rox_net::providers::search_art(&query) {
                                Ok(candidates) => {
                                    // The art searches are fuzzy, and trusting provider order
                                    // picked the first Deezer hit even when it was a different
                                    // album entirely (issue #79). Score every candidate against
                                    // the track and take the closest, provider preference only
                                    // breaking ties. Below the bar nothing came close enough,
                                    // and the app icon beats someone else's cover.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped lines against a fully tagged track: the card a fresh
    /// install publishes, and the line the member list repeats.
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

    /// A tag the track doesn't carry takes its separator with it, and a
    /// line with nothing left in it comes out empty so the card leaves it
    /// off. Stations are the common case: plenty send a title and nothing
    /// else, and " - " under the name is worse than one line.
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

    /// %format% reads as the phrase the artwork's hover text uses, so the
    /// two can't drift.
    #[test]
    fn the_format_placeholder_reads_as_the_quality_phrase() {
        let sample = PresenceSample::default();

        assert_eq!(preview("%format%", &sample).unwrap(), "FLAC Lossless");
        assert_eq!(
            preview("%title% • %format%", &sample).unwrap(),
            "Xtal • FLAC Lossless"
        );
    }

    /// A typo is refused rather than published as literal text, and the
    /// card falls back to the default line instead of blanking.
    #[test]
    fn a_typoed_placeholder_is_refused_and_falls_back() {
        assert!(preview("%tittle%", &PresenceSample::default()).is_err());
        assert_eq!(
            line("%tittle%", DEFAULT_PRESENCE_FIRST_LINE)
                .render_line(&PresenceSample::default().values()),
            "Aphex Twin - Xtal"
        );
    }

    /// A blank line is how a line is turned off: it parses, renders
    /// nothing, and the card goes out without the field.
    #[test]
    fn a_blank_line_renders_nothing() {
        assert_eq!(preview("", &PresenceSample::default()).unwrap(), "");
    }
}
