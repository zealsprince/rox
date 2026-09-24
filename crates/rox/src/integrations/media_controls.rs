//! OS media controls: MPRIS on Linux, SMTC on Windows, the remote command
//! center on macOS. Answers the hardware media keys and shows the playing
//! track in the desktop's media widget.
//!
//! Key presses arrive on souvlaki's thread and cross to the UI over a
//! channel; state goes back out on the player observer, gated so frame
//! notifies don't become D-Bus writes. [`MediaSession`] is its own entity so
//! the service outlives its window and keeps working from the tray.

use std::path::Path;
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, Subscription, Task, Window};
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};

use rox_core::APP_ID;
use rox_library::cue::TrackKey;
use rox_library::hash::fnv1a;
use rox_panel_api::panel::AppState;
use rox_services::player::NowPlaying;

/// Play and Pause stay distinct from Toggle so the OS buttons hit the right
/// transition instead of flipping whatever state we're in.
pub enum MediaCommand {
    Toggle,
    Play,
    Pause,
    Next,
    Prev,
    Stop,
    /// Signed seconds, forward positive.
    SeekBy(f64),
    SeekTo(f64),
}

/// A bare Seek with no distance, matching the arrow-key binding.
const SEEK_STEP: f64 = 5.0;

/// Drift from the extrapolated playhead that counts as a seek. Above
/// notify-cadence jitter, well under any real seek.
const SEEK_EPSILON: Duration = Duration::from_millis(1000);

/// Dropping it tears the media service down.
pub struct MediaKeys {
    controls: MediaControls,
    events: async_channel::Receiver<MediaCommand>,
    /// Last pushed play state: `None` stopped, `Some(playing)` with a track loaded.
    state: Option<bool>,
    /// Forces the next play-state push after a track change so the widget's
    /// progress resets. Separate from `state` so a stop isn't read as the force.
    force: bool,
    /// Kept so a late cover can re-emit the whole block: souvlaki writes every
    /// field in one `set_metadata`.
    meta: Option<NowPlayingMeta>,
    cover: Option<String>,
    /// The seek baseline. MPRIS clients extrapolate from the last pushed
    /// progress, so a seek has to be pushed even when the play state holds.
    pushed_position: Option<Duration>,
    pushed_at: Option<Instant>,
}

pub struct NowPlayingMeta {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Option<Duration>,
}

impl MediaKeys {
    /// `None` when the backend won't come up, so the app runs on without media keys.
    pub fn new(window: &Window) -> Option<MediaKeys> {
        let hwnd = window_hwnd(window);
        // souvlaki's SMTC backend panics without an HWND.
        #[cfg(target_os = "windows")]
        if hwnd.is_none() {
            return None;
        }
        // souvlaki's zbus thread unwraps the name request, so a second instance on
        // the same name panics with NameTaken. MPRIS allows a per-instance suffix,
        // which controllers still match on the prefix.
        let dbus_name = format!("{APP_ID}.instance{}", std::process::id());
        let config = PlatformConfig {
            dbus_name: &dbus_name,
            display_name: "rox",
            hwnd,
        };
        let mut controls = MediaControls::new(config).ok()?;
        let (tx, events) = async_channel::unbounded();
        controls
            .attach(move |event| {
                // Souvlaki's thread: map and hand to the UI.
                if let Some(cmd) = interpret(event) {
                    let _ = tx.try_send(cmd);
                }
            })
            .ok()?;
        Some(MediaKeys {
            controls,
            events,
            state: None,
            force: false,
            meta: None,
            cover: None,
            pushed_position: None,
            pushed_at: None,
        })
    }

    pub fn events(&self) -> async_channel::Receiver<MediaCommand> {
        self.events.clone()
    }

    /// Called only on track turnover. Drops the cover, which arrives later via
    /// [`set_cover`](Self::set_cover).
    pub fn set_track(&mut self, meta: Option<NowPlayingMeta>) {
        self.meta = meta;
        self.cover = None;
        self.emit();
        self.force = true;
    }

    pub fn set_cover(&mut self, url: Option<String>) {
        self.cover = url;
        self.emit();
    }

    fn emit(&mut self) {
        let _ = self.controls.set_metadata(match &self.meta {
            Some(m) => MediaMetadata {
                title: Some(&m.title),
                artist: Some(&m.artist),
                album: Some(&m.album),
                duration: m.duration,
                cover_url: self.cover.as_deref(),
            },
            None => MediaMetadata::default(),
        });
    }

    pub fn set_playing(&mut self, has_track: bool, playing: bool, position: Option<Duration>) {
        let state = has_track.then_some(playing);
        // A seek leaves the state alone, so push when the position jumped too.
        let seeked = self.position_jumped(position);
        if !self.force && self.state == state && !seeked {
            return;
        }
        self.force = false;
        self.state = state;
        self.pushed_position = position;
        self.pushed_at = Some(Instant::now());
        let progress = position.map(MediaPosition);
        let _ = self.controls.set_playback(match state {
            None => MediaPlayback::Stopped,
            Some(true) => MediaPlayback::Playing { progress },
            Some(false) => MediaPlayback::Paused { progress },
        });
    }

    /// The baseline only advances while the last pushed state was playing, so a
    /// scrub while paused counts too.
    fn position_jumped(&self, position: Option<Duration>) -> bool {
        let (Some(pos), Some(base), Some(at)) = (position, self.pushed_position, self.pushed_at)
        else {
            return false;
        };
        let advanced = if self.state == Some(true) {
            at.elapsed()
        } else {
            Duration::ZERO
        };
        let expected = base + advanced;
        pos.abs_diff(expected) > SEEK_EPSILON
    }
}

/// The media service bound to a player. An entity so it lives as long as the
/// service, not a window: one per process, on the primary window or in the
/// tray's hold.
pub struct MediaSession {
    keys: MediaKeys,
    state: AppState,
    /// So the library resolve only runs on a track change.
    track: Option<TrackKey>,
    /// A stream changes song without the key moving.
    live_rev: Option<u64>,
    _player: Subscription,
    _events: Task<()>,
}

impl MediaSession {
    pub fn new(state: AppState, window: &Window, cx: &mut App) -> Option<Entity<MediaSession>> {
        let keys = MediaKeys::new(window)?;
        Some(cx.new(|cx| {
            let events = keys.events();
            let mut session = MediaSession {
                keys,
                _player: cx.observe(&state.player, |this: &mut MediaSession, _, cx| {
                    this.publish(cx)
                }),
                _events: cx.spawn(async move |this, cx| {
                    while let Ok(cmd) = events.recv().await {
                        let applied = this.update(cx, |this, cx| {
                            this.apply(cmd, cx);
                            this.publish(cx);
                        });
                        if applied.is_err() {
                            break;
                        }
                    }
                }),
                state,
                track: None,
                live_rev: None,
            };
            // Seed now: a hand-off arrives mid-track, and a paused player may not notify soon.
            session.publish(cx);
            session
        }))
    }

    fn apply(&mut self, cmd: MediaCommand, cx: &mut Context<Self>) {
        self.state.player.update(cx, |player, cx| match cmd {
            MediaCommand::Toggle => player.toggle_pause(),
            MediaCommand::Play => {
                if !player.is_playing() {
                    player.toggle_pause();
                }
            }
            MediaCommand::Pause => {
                if player.is_playing() {
                    player.toggle_pause();
                }
            }
            MediaCommand::Next => player.next(cx),
            MediaCommand::Prev => player.prev(),
            MediaCommand::Stop => player.stop(cx),
            MediaCommand::SeekBy(delta) => player.seek_by(delta),
            MediaCommand::SeekTo(secs) => player.seek_to(secs),
        });
    }

    fn publish(&mut self, cx: &mut Context<Self>) {
        let now = self.state.player.read(cx).now_playing();
        let playing = self.state.player.read(cx).is_playing();
        let live_rev = self.state.player.read(cx).title_rev();
        // The whole key, not the path: two cue tracks share an image.
        let key = now.as_ref().map(|now| now.key.clone());
        if key != self.track || live_rev != self.live_rev {
            let moved = key != self.track;
            self.track = key.clone();
            self.live_rev = live_rev;
            let meta = now.as_ref().map(|now| self.now_playing_meta(now, cx));
            self.keys.set_track(meta);
            // A station's next song keeps the same art.
            if moved {
                self.publish_cover(key.clone(), cx);
            }
        }
        let position = now
            .as_ref()
            .map(|now| Duration::from_secs_f64(now.position_secs.max(0.0)));
        self.keys.set_playing(key.is_some(), playing, position);
    }

    /// A result landing after the track moved on is dropped. A row with no file
    /// reads the thumbnail store; a station shows its logo, not the song's art.
    fn publish_cover(&mut self, track: Option<TrackKey>, cx: &mut Context<Self>) {
        let Some(track) = track else {
            return;
        };
        let remote = !track.is_local();
        let thumbs = remote
            .then(|| self.state.thumbs.read(cx).store_conn())
            .flatten();
        cx.spawn(async move |this, cx| {
            let resolved = track.path.clone();
            let cover = cx
                .background_executor()
                .spawn(async move {
                    let art = match thumbs {
                        Some(thumbs) => {
                            rox_services::sources::art(&thumbs, &resolved.to_string_lossy())
                                .map(|bytes| (bytes, "image/jpeg".to_string()))
                        }
                        None if remote => None,
                        None => rox_library::art::cover_art(&resolved),
                    };
                    art.and_then(|(bytes, mime)| cache_now_playing_art(&resolved, &bytes, &mime))
                })
                .await;
            this.update(cx, |this, _| {
                if this.track.as_ref() != Some(&track) {
                    return;
                }
                this.keys.set_cover(cover);
            })
            .ok();
        })
        .detach();
    }

    fn now_playing_meta(&self, now: &NowPlaying, cx: &App) -> NowPlayingMeta {
        // Through the player, so a station shows the song it announced.
        let player = self.state.player.read(cx);
        let tags = player.live_over(self.state.library.read(cx).meta_for_key(&now.key));
        let title = tags
            .as_ref()
            .map(|m| m.title.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| {
                // A remote track shows an empty title rather than a source's id.
                now.path()
                    .and_then(|path| path.file_stem())
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
        NowPlayingMeta {
            title,
            artist: tags.as_ref().map(|m| m.artist.clone()).unwrap_or_default(),
            album: tags.map(|m| m.album).unwrap_or_default(),
            duration: now
                .duration_secs
                .filter(|d| *d > 0.0)
                .map(Duration::from_secs_f64),
        }
    }
}

fn interpret(event: MediaControlEvent) -> Option<MediaCommand> {
    Some(match event {
        MediaControlEvent::Play => MediaCommand::Play,
        MediaControlEvent::Pause => MediaCommand::Pause,
        MediaControlEvent::Toggle => MediaCommand::Toggle,
        MediaControlEvent::Next => MediaCommand::Next,
        MediaControlEvent::Previous => MediaCommand::Prev,
        MediaControlEvent::Stop => MediaCommand::Stop,
        MediaControlEvent::Seek(dir) => MediaCommand::SeekBy(signed(dir, SEEK_STEP)),
        MediaControlEvent::SeekBy(dir, by) => MediaCommand::SeekBy(signed(dir, by.as_secs_f64())),
        MediaControlEvent::SetPosition(pos) => MediaCommand::SeekTo(pos.0.as_secs_f64()),
        _ => return None,
    })
}

fn signed(dir: SeekDirection, secs: f64) -> f64 {
    match dir {
        SeekDirection::Forward => secs,
        SeekDirection::Backward => -secs,
    }
}

#[cfg(target_os = "windows")]
fn window_hwnd(window: &Window) -> Option<*mut std::ffi::c_void> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    // gpui's inherent window_handle() shadows the trait method, so call it
    // through the trait.
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as *mut std::ffi::c_void),
        _ => None,
    }
}

#[cfg(not(target_os = "windows"))]
fn window_hwnd(_window: &Window) -> Option<*mut std::ffi::c_void> {
    None
}

/// Write the cover to a scratch file and return its `file://` URL: souvlaki
/// takes a URL on every platform. Blocking. Named by the track, and every
/// other file in the directory is pruned, so only the current cover stays.
pub fn cache_now_playing_art(track: &Path, bytes: &[u8], mime: &str) -> Option<String> {
    let dir = rox_core::settings::data_dir().join("nowplaying");
    std::fs::create_dir_all(&dir).ok()?;
    let name = format!(
        "{:016x}.{}",
        fnv1a(track.as_os_str().as_encoded_bytes()),
        mime_ext(mime)
    );
    let file = dir.join(&name);
    std::fs::write(&file, bytes).ok()?;
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        if entry.path() != file {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    url::Url::from_file_path(&file).ok().map(|u| u.to_string())
}

/// Cosmetic: every platform sniffs the bytes.
fn mime_ext(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        _ => "img",
    }
}
