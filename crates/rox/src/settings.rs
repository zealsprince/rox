//! Persisted app settings: one JSON file in the app's data directory, next
//! to the library database. Writers each own a few fields (the player its
//! playback state, the workspace its window and layout) and write through
//! [`Settings::update`], which reloads the file first so one writer's save
//! never reverts another's fields to what they were at startup.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use rox_playback::engine::LoopMode;

/// The app's data directory, shared with the library database. Created on
/// first use.
pub fn data_dir() -> PathBuf {
    let dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rox");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn settings_path() -> PathBuf {
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
    /// Loop mode as its wire name: "off", "all", or "one". The engine's
    /// `LoopMode` stays serde-free; convert through the accessors.
    pub loop_mode: String,
    /// The main window's last frame, restored on open. None until the first
    /// window closes.
    pub window: Option<WindowState>,
    /// The dock layout as the dock crate's own serialized state, kept as raw
    /// JSON so settings stay readable even when the layout schema moves; the
    /// workspace validates and versions it on restore. None until a layout
    /// has been saved.
    pub layout: Option<serde_json::Value>,
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
            loop_mode: "off".into(),
            window: None,
            layout: None,
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
