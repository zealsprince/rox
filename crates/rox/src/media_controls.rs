//! OS media controls: one MPRIS service on Linux (SMTC on Windows, the remote
//! command center on macOS) that answers the hardware media keys and shows the
//! now-playing track in the desktop's media widget. The D-Bus name is
//! per-process, so this is wired to the primary workspace only.
//!
//! Windows' SMTC binds to a window, so [`MediaKeys::new`] takes the primary
//! workspace window and hands its HWND down; the other two backends ignore it.
//!
//! Two directions cross the thread boundary here. Key presses arrive on
//! souvlaki's own event-loop thread; the attach callback maps each one to a
//! [`MediaCommand`] and hands it to the UI over an async channel the workspace
//! awaits, so there is no poll. State and metadata go the other way: the
//! workspace pushes the playing track and play state back out on the player
//! observer, and the gating here keeps a steady stream of frame notifies from
//! turning into a stream of D-Bus writes.

use std::path::Path;
use std::time::Duration;

use gpui::Window;
use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};

use crate::APP_ID;

/// A media-key press mapped off souvlaki's own event vocabulary onto the
/// transport verbs the player already speaks. Play and Pause stay distinct
/// from Toggle so the OS "play" and "pause" buttons land on the right edge
/// instead of flipping whatever state we happen to be in.
pub enum MediaCommand {
    Toggle,
    Play,
    Pause,
    Next,
    Prev,
    Stop,
    /// Relative seek in seconds, signed. Forward is positive.
    SeekBy(f64),
    /// Absolute seek to a position in seconds.
    SeekTo(f64),
}

/// How far a bare Seek (no distance given) jumps, matching the arrow-key
/// binding in the workspace.
const SEEK_STEP: f64 = 5.0;

/// The souvlaki handle plus the receiver its callback feeds. Kept alive for
/// the whole session: dropping it tears the media service down and ends the
/// event stream.
pub struct MediaKeys {
    controls: MediaControls,
    events: async_channel::Receiver<MediaCommand>,
    /// The play state last written out, so a same-state notify (the player
    /// pump fires one every frame while audio moves) does not write again.
    /// `None` means stopped, `Some(playing)` means a track is loaded.
    state: Option<bool>,
    /// Set by a track change to push the next play-state write through even
    /// when the state itself hasn't moved, so the widget's progress resets to
    /// the new track. Kept apart from `state` so a stop (state -> `None`)
    /// isn't mistaken for the force sentinel.
    force: bool,
    /// The current track's tags, kept so a cover that resolves after the text
    /// can re-emit the metadata whole - souvlaki writes every field in one
    /// `set_metadata`, so a late cover can't be pushed on its own.
    meta: Option<NowPlayingMeta>,
    /// The `file://` URL of the current track's cached cover. `None` until the
    /// art resolves, and while a track carries none.
    cover: Option<String>,
}

/// The now-playing tags the widget shows, resolved by the workspace off the
/// library so this module stays clear of the catalog.
pub struct NowPlayingMeta {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Option<Duration>,
}

impl MediaKeys {
    /// Register the media service and start listening. `None` when the
    /// platform backend won't come up (no session bus, say), so the app runs
    /// on without media keys rather than failing to launch. Takes the primary
    /// window because Windows' SMTC binds to its HWND.
    pub fn new(window: &Window) -> Option<MediaKeys> {
        let hwnd = window_hwnd(window);
        // souvlaki's SMTC backend panics without an HWND, so if we couldn't
        // pull one, skip the service and run on rather than crash the launch.
        // Off Windows this never trips: the field is ignored and stays `None`.
        #[cfg(target_os = "windows")]
        if hwnd.is_none() {
            return None;
        }
        let config = PlatformConfig {
            dbus_name: APP_ID,
            display_name: "rox",
            // Windows SMTC binds to this window; Linux and macOS ignore it.
            hwnd,
        };
        let mut controls = MediaControls::new(config).ok()?;
        let (tx, events) = async_channel::unbounded();
        controls
            .attach(move |event| {
                // Runs on souvlaki's event-loop thread. Map to a transport
                // verb and hand it to the UI; drop the ones we don't wire.
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
        })
    }

    /// A receiver clone for the workspace's await loop.
    pub fn events(&self) -> async_channel::Receiver<MediaCommand> {
        self.events.clone()
    }

    /// Push the now-playing tags to the widget. Called only when the track
    /// turns over, so the resolve behind it stays off the frame path. A
    /// `None` clears the widget back to nothing playing. The cover is dropped
    /// here and pushed later through [`set_cover`](Self::set_cover), since it
    /// resolves off the UI thread and lands after the text.
    pub fn set_track(&mut self, meta: Option<NowPlayingMeta>) {
        self.meta = meta;
        self.cover = None;
        self.emit();
        // A fresh track forces the next play-state push through so the
        // widget's progress resets to the new track even if it was already
        // playing.
        self.force = true;
    }

    /// Attach the resolved cover to the current track and re-emit. The
    /// workspace resolves art off the UI thread and calls this when it lands,
    /// guarded so a cover only reaches the track it belongs to. `None` leaves
    /// the widget coverless (the track has none, or the read failed).
    pub fn set_cover(&mut self, url: Option<String>) {
        self.cover = url;
        self.emit();
    }

    /// Write the whole metadata block out. souvlaki takes every field in one
    /// `set_metadata`, so the text and the cover ride together each time.
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

    /// Push the play state, gated so an unchanged state writes nothing. A
    /// track with no session behind it reads as stopped.
    pub fn set_playing(&mut self, has_track: bool, playing: bool, position: Option<Duration>) {
        let state = has_track.then_some(playing);
        if !self.force && self.state == state {
            return;
        }
        self.force = false;
        self.state = state;
        let progress = position.map(MediaPosition);
        let _ = self.controls.set_playback(match state {
            None => MediaPlayback::Stopped,
            Some(true) => MediaPlayback::Playing { progress },
            Some(false) => MediaPlayback::Paused { progress },
        });
    }
}

/// Map one souvlaki event onto a transport verb, or `None` for the events we
/// don't act on (raise, quit, open-uri, volume).
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

/// A seek distance signed by its direction: forward positive, backward
/// negative.
fn signed(dir: SeekDirection, secs: f64) -> f64 {
    match dir {
        SeekDirection::Forward => secs,
        SeekDirection::Backward => -secs,
    }
}

/// The Win32 HWND souvlaki's SMTC backend binds to, pulled off the gpui
/// window. Windows needs it; every other backend ignores the field, so this
/// is `None` off Windows and the window goes unread there.
#[cfg(target_os = "windows")]
fn window_hwnd(window: &Window) -> Option<*mut std::ffi::c_void> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as *mut std::ffi::c_void),
        _ => None,
    }
}

#[cfg(not(target_os = "windows"))]
fn window_hwnd(_window: &Window) -> Option<*mut std::ffi::c_void> {
    None
}

/// Stash the now-playing cover to a scratch file and hand back its `file://`
/// URL for the transport widget. souvlaki wants a URL, not bytes, on every
/// platform: MPRIS forwards it as `mpris:artUrl`, and SMTC and the macOS
/// center load the file themselves. Blocking file writes; run it off the UI
/// thread.
///
/// The file is named by the track so its URL stays valid while the track is
/// up, and every other file in the directory is pruned on write, so the
/// scratch dir never holds more than the current cover.
pub fn cache_now_playing_art(track: &Path, bytes: &[u8], mime: &str) -> Option<String> {
    let dir = crate::settings::data_dir().join("nowplaying");
    std::fs::create_dir_all(&dir).ok()?;
    let name = format!(
        "{:016x}.{}",
        fnv1a(track.as_os_str().as_encoded_bytes()),
        mime_ext(mime)
    );
    let file = dir.join(&name);
    std::fs::write(&file, bytes).ok()?;
    // Drop the previous track's cover; only the current one is advertised, so
    // nothing is still reading the stale URL by the time we get here.
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        if entry.path() != file {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    url::Url::from_file_path(&file).ok().map(|u| u.to_string())
}

/// The file extension for a cover mime. Cosmetic - every platform sniffs the
/// bytes rather than trusting the name - but a right extension keeps the
/// scratch file honest. Unknown mimes fall back to a bare `img`.
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

/// FNV-1a over the track path, stable across runs so the same track keeps its
/// scratch filename. Matches the waveform cache's keying.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}
