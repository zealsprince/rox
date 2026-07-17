//! Persisted app settings: one JSON file in the app's data directory, next
//! to the library database. Writers each own a few fields (the player its
//! playback state, the workspace its window and layout) and write through
//! [`Settings::update`], which reloads the file first so one writer's save
//! never reverts another's fields to what they were at startup.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use rox_playback::engine::LoopMode;

use crate::design::palette::Palette;

/// The app's data directory, shared with the library database. Created on
/// first use.
pub fn data_dir() -> PathBuf {
    let dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rox");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// The settings file inside [`data_dir`], public so the settings window
/// can hand the raw file to the system editor.
pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

/// Everything the app persists outside the library database. Unknown fields
/// are dropped on load and missing ones take defaults, so the file survives
/// version drift in both directions.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Linear playback volume, same range the engine clamps to (0 to 2).
    pub volume: f32,
    /// Whether output is muted. The volume above is the level mute returns
    /// to, so muting never loses the setting.
    pub muted: bool,
    /// Loop mode as its wire name: "off", "all", or "one". The engine's
    /// `LoopMode` stays serde-free; convert through the accessors.
    pub loop_mode: String,
    /// Whether playback shuffles: the queue plays in a random order
    /// instead of front to back.
    pub shuffle: bool,
    /// The main window's last frame, restored on open. None until the first
    /// window closes.
    pub window: Option<WindowState>,
    /// The dock layout as the dock crate's own serialized state, kept as raw
    /// JSON so settings stay readable even when the layout schema moves; the
    /// workspace validates and versions it on restore. None until a layout
    /// has been saved.
    pub layout: Option<serde_json::Value>,
    /// The folders the library scans, in the order they were added. Empty
    /// until one has been opened.
    pub library_roots: Vec<PathBuf>,
    /// The single folder `library_roots` replaced. Read once on load to
    /// seed the list, never written back.
    #[serde(skip_serializing)]
    library_root: Option<PathBuf>,
    /// ADR 10's transparency pair, both 0 to 1. How opaque the app's
    /// surfaces read, 1 fully opaque...
    pub surface_opacity: f32,
    /// ...and how strongly the backdrop shows behind them, 1 the bare
    /// bake, 0 sunk into the floor.
    pub backdrop_strength: f32,
    /// The user palette as role-name-to-`#rrggbb` entries,
    /// [`Palette::to_map`]'s shape. Empty means the default palette;
    /// unknown roles fall away on load, like the file's own fields.
    pub palette: BTreeMap<String, String>,
    /// Whether the playing track's art re-tints the palette and backs
    /// the windows (ADR 10's derived mode). Off by default: the look
    /// only follows the music when asked to.
    pub art_theming: bool,
    /// Whether launch loads the last playing track back up, paused where
    /// it left off. The track below is written either way; this only
    /// gates the restore.
    pub restore_last_track: bool,
    /// What was playing when the app closed, as a library track id so it
    /// survives path changes, plus where the clock sat. None when nothing
    /// was playing; a stale id degrades to the cold start on restore.
    pub last_track: Option<LastTrack>,
    /// The last.fm connection and scrobbling knobs, the settings window's
    /// Scrobbling page.
    pub lastfm: Lastfm,
    /// The quick-play modal's appearance knobs, edited from its own config
    /// panel.
    pub quick_play: QuickPlayConfig,
}

/// How the quick-play modal draws its result list, the knobs its inline
/// config panel edits. Persisted so the look survives reopening the modal,
/// which the workspace rebuilds each time.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QuickPlayConfig {
    /// Show the artist and album line under each result's title.
    pub show_subtitle: bool,
    /// Show each result's duration on the right.
    pub show_duration: bool,
    /// Give each result row more height.
    pub comfortable: bool,
}

impl Default for QuickPlayConfig {
    fn default() -> Self {
        QuickPlayConfig {
            show_subtitle: true,
            show_duration: true,
            comfortable: false,
        }
    }
}

/// The last.fm account and how scrobbling behaves. The key and secret
/// override the build's own api identity (`lastfm::keys`), for builds
/// that ship none; the session key is what the connect flow lands and
/// never expires until revoked on last.fm.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Lastfm {
    pub api_key: String,
    pub api_secret: String,
    pub session_key: String,
    /// The account the session belongs to, for the settings readout.
    pub username: String,
    /// Whether playback scrobbles at all; the connection stays either way.
    pub scrobbling: bool,
    /// How much of a track has to actually play before it scrobbles, as a
    /// fraction of its duration. The seek strip and waveform can mark it.
    pub threshold: f32,
}

impl Default for Lastfm {
    fn default() -> Self {
        Lastfm {
            api_key: String::new(),
            api_secret: String::new(),
            session_key: String::new(),
            username: String::new(),
            scrobbling: true,
            threshold: 0.5,
        }
    }
}

/// The closing snapshot of the playing track: its library id and the
/// position clock in seconds.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct LastTrack {
    pub id: i64,
    pub position_secs: f64,
}

/// A window frame in logical pixels, plus whether the window was maximized
/// (the frame is then the restore size).
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct WindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            volume: 1.0,
            muted: false,
            loop_mode: "off".into(),
            shuffle: false,
            window: None,
            layout: None,
            library_roots: Vec::new(),
            library_root: None,
            surface_opacity: 1.0,
            backdrop_strength: 1.0,
            palette: BTreeMap::new(),
            art_theming: false,
            restore_last_track: true,
            last_track: None,
            lastfm: Lastfm::default(),
            quick_play: QuickPlayConfig::default(),
        }
    }
}

impl Settings {
    /// Read the settings file, falling back to defaults if it is missing or
    /// unreadable. A corrupt file logs and resets rather than blocking start.
    pub fn load() -> Settings {
        let path = settings_path();
        let mut settings: Settings = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                eprintln!("settings: resetting {}: {e}", path.display());
                Settings::default()
            }),
            Err(_) => Settings::default(),
        };
        // A hand-edited volume seeds the engine's atomics directly, so the
        // engine's clamp range applies here too.
        settings.volume = if settings.volume.is_finite() {
            settings.volume.clamp(0.0, 2.0)
        } else {
            1.0
        };
        // The transparency pair reads straight into color math, so
        // hand-edited values clamp to the unit range.
        for scalar in [
            &mut settings.surface_opacity,
            &mut settings.backdrop_strength,
        ] {
            *scalar = if scalar.is_finite() {
                scalar.clamp(0.0, 1.0)
            } else {
                1.0
            };
        }
        // The threshold reads straight into the scrobble math and the
        // marker paint, so a hand-edited value clamps to a sane band.
        settings.lastfm.threshold = if settings.lastfm.threshold.is_finite() {
            settings.lastfm.threshold.clamp(0.1, 1.0)
        } else {
            0.5
        };
        // A file from before multi-folder carries one library_root; it
        // seeds the list here and the next save drops it.
        if settings.library_roots.is_empty() {
            if let Some(root) = settings.library_root.take() {
                settings.library_roots.push(root);
            }
        }
        settings
    }

    /// Change some fields and persist: reload the file, apply, write it
    /// back. Writers hold their own in-memory copies for reads, so going
    /// through the file here is what keeps one writer's save from
    /// reverting another's fields.
    pub fn update(f: impl FnOnce(&mut Settings)) {
        let mut settings = Settings::load();
        f(&mut settings);
        settings.save();
    }

    /// Write the whole file. Failures log and move on; settings loss is not
    /// worth interrupting playback for.
    fn save(&self) {
        let path = settings_path();
        let text = serde_json::to_string_pretty(self).expect("settings serialize");
        if let Err(e) = std::fs::write(&path, text) {
            eprintln!("settings: writing {}: {e}", path.display());
        }
    }

    /// The user palette the map holds, over the defaults.
    pub fn palette(&self) -> Palette {
        Palette::from_map(&self.palette)
    }

    pub fn loop_mode(&self) -> LoopMode {
        match self.loop_mode.as_str() {
            "all" => LoopMode::All,
            "one" => LoopMode::One,
            _ => LoopMode::Off,
        }
    }

    pub fn set_loop_mode(&mut self, mode: LoopMode) {
        self.loop_mode = match mode {
            LoopMode::Off => "off",
            LoopMode::All => "all",
            LoopMode::One => "one",
        }
        .into();
    }
}
