//! Persisted app settings in the data directory. `settings.json` holds the
//! preferences and library setup, small enough to hand-edit. `workspace.json`
//! holds the live look, `windows.json`, `session.json`, and `accounts.json` a
//! shard each, and `workspaces/` the saved workspaces, one exported bundle per
//! file. Writers go through [`Settings::update`], which reloads first so one
//! writer's save never reverts another's fields.
//!
//! `layouts` holds the named dock presets, `panel_presets` the named single
//! panels. The settings window lives up in rox, with the widgets.

pub mod layouts;
pub mod panel_presets;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{LazyLock, OnceLock, RwLock};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use gpui::{App, SharedString, WindowAppearance, WindowDecorations, px};
use serde::{Deserialize, Serialize};

use rox_playback::engine::LoopMode;
use rox_viz::signal::{Route, Signal};

use rox_design::palette::{self, Palette, Sides};

use crate::acoustic;
use crate::continuation;
use crate::install;
use crate::pattern::{self, Pattern, PatternField};

/// The OS minimum and the clamp programmatic resizes run through, so a bad
/// stored preset size can't collapse a window to nothing. Kept low: the
/// dock's per-panel minimums usually stop a resize first.
pub const MIN_WINDOW_SIZE: gpui::Size<gpui::Pixels> = gpui::Size {
    width: px(20.),
    height: px(20.),
};

/// Where a pre-split settings file's workspaces go. Startup installs it
/// before anything reads a setting.
static WORKSPACE_MIGRATOR: OnceLock<fn(WorkspaceBundle)> = OnceLock::new();

pub fn set_workspace_migrator(migrate: fn(WorkspaceBundle)) {
    let _ = WORKSPACE_MIGRATOR.set(migrate);
}

/// Under an AppImage this is the folder holding the .AppImage, since the
/// mount the binary runs from is read-only and gone after exit.
fn exe_dir() -> Option<PathBuf> {
    if let Some(dir) = install::appimage().and_then(Path::parent) {
        return Some(dir.to_path_buf());
    }

    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// The marker file beside the executable that keeps portable mode on.
pub fn portable_marker() -> Option<PathBuf> {
    exe_dir().map(|dir| dir.join("portable"))
}

const PORTABLE_DATA: &str = "rox-data";

pub fn portable_data_dir() -> Option<PathBuf> {
    exe_dir().map(|dir| dir.join(PORTABLE_DATA))
}

/// The data root and whether it's portable, decided once per process so a
/// mid-run toggle can't split the stores.
static DATA_DIR: OnceLock<(PathBuf, bool)> = OnceLock::new();

fn resolve_data_dir() -> (PathBuf, bool) {
    // `--fresh` wipes a scratch data dir so each launch is a real first run.
    // Debug builds only, so it never becomes user-facing.
    if cfg!(debug_assertions) && std::env::args().any(|arg| arg == "--fresh") {
        let dir = std::env::temp_dir().join("rox-fresh");
        let _ = std::fs::remove_dir_all(&dir);
        return (dir, false);
    }

    let portable = std::env::args().any(|arg| arg == "--portable")
        || portable_marker().is_some_and(|marker| marker.exists());
    choose_data_dir(portable, exe_dir().as_deref())
}

fn choose_data_dir(portable: bool, exe_dir: Option<&Path>) -> (PathBuf, bool) {
    if portable {
        match exe_dir {
            Some(dir) if dir_writable(dir) => return (dir.join(PORTABLE_DATA), true),
            Some(dir) => log::warn!(
                "portable mode requested, but {} takes no writes; using the OS data dir",
                dir.display()
            ),
            None => log::warn!(
                "portable mode requested, but the executable's folder is unknown; using the OS data dir"
            ),
        }
    }

    let dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rox");
    (dir, false)
}

pub fn data_dir() -> PathBuf {
    let (dir, _) = DATA_DIR.get_or_init(resolve_data_dir);
    let _ = std::fs::create_dir_all(dir);
    dir.clone()
}

pub fn portable() -> bool {
    DATA_DIR.get_or_init(resolve_data_dir).1
}

/// Whether the executable's folder takes writes. Probes with a real file,
/// since permission reads aren't reliable across platforms.
pub fn portable_available() -> bool {
    exe_dir().is_some_and(|dir| dir_writable(&dir))
}

fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(".rox-write-probe");

    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Whether this launch found no settings file. [`note_first_run`] records it
/// at startup, before anything can write the file.
static FIRST_RUN: AtomicBool = AtomicBool::new(false);

pub fn note_first_run() {
    FIRST_RUN.store(!settings_path().exists(), Ordering::Relaxed);
}

pub fn first_run() -> bool {
    FIRST_RUN.load(Ordering::Relaxed)
}

pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

pub fn look_path() -> PathBuf {
    data_dir().join("workspace.json")
}

pub fn windows_path() -> PathBuf {
    data_dir().join("windows.json")
}

pub fn session_path() -> PathBuf {
    data_dir().join("session.json")
}

/// Account keys get their own file so the settings file people hand around
/// holds no credentials.
pub fn accounts_path() -> PathBuf {
    data_dir().join("accounts.json")
}

pub fn workspaces_dir() -> PathBuf {
    data_dir().join("workspaces")
}

/// Ejected shaders, one subfolder per workspace. The first eject creates it.
pub fn shaders_dir() -> PathBuf {
    data_dir().join("shaders")
}

/// Milkdrop `presets/` and `textures/`. Not created here: users unzip packs
/// into it themselves (ADR 28).
pub fn milkdrop_dir() -> PathBuf {
    data_dir().join("milkdrop")
}

/// An unsaved look ejects under `_local`. A workspace named that shares the
/// folder, which costs at worst a bookmark, since the re-link checks the hash.
pub fn shader_eject_path(workspace: &str, shader: &str) -> PathBuf {
    shader_eject_path_in(&shaders_dir(), workspace, shader)
}

pub fn shader_eject_path_in(root: &Path, workspace: &str, shader: &str) -> PathBuf {
    root.join(safe_file_stem(workspace, "_local"))
        .join(format!("{}.wgsl", safe_file_stem(shader, "shader")))
}

/// A name as a file or folder name: separators, Windows-reserved and control
/// characters fold to spaces, and leading dots go so the file isn't hidden.
pub fn safe_file_stem(name: &str, fallback: &str) -> String {
    let folded: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let stem = folded.trim().trim_matches('.').trim();
    if stem.is_empty() {
        fallback.to_string()
    } else {
        stem.to_string()
    }
}

/// Write pretty JSON through a sibling temp file and rename, so a crash
/// mid-write can't truncate the file. Failures log under `what` and move on.
pub fn write_json<T: Serialize>(path: &Path, value: &T, what: &str) -> bool {
    let text = match serde_json::to_string_pretty(value) {
        Ok(text) => text,
        // A non-finite f32 fails here. Keep the old file rather than panic.
        Err(e) => {
            log::warn!("{what}: serializing: {e}");
            return false;
        }
    };
    if let Some(dir) = path.parent()
        && let Err(e) = std::fs::create_dir_all(dir)
    {
        log::warn!("{what}: creating {}: {e}", dir.display());
        return false;
    }
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, &text) {
        log::warn!("{what}: writing {}: {e}", tmp.display());
        return false;
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        log::warn!("{what}: replacing {}: {e}", path.display());
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    true
}

/// Deserializers that drop only the broken piece of a file, where serde's
/// default would reset the whole file over one bad entry. Each logs what it
/// drops.
///
/// Which one a field takes matters. Independent lists drop the bad entry. A
/// queue's `cursor` indexes its `entries`, so the queue fails whole as an
/// option: dropping one entry would resume the wrong track. Closed word sets
/// (modes, styles) take the default, since an unknown word is likely a newer
/// build's spelling.
mod lenient {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer};

    pub fn vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: serde::de::DeserializeOwned,
    {
        let raw = Vec::<serde_json::Value>::deserialize(deserializer)?;
        Ok(raw.into_iter().filter_map(parse).collect())
    }

    pub fn map<'de, D, T>(deserializer: D) -> Result<BTreeMap<String, T>, D::Error>
    where
        D: Deserializer<'de>,
        T: serde::de::DeserializeOwned,
    {
        let raw = BTreeMap::<String, serde_json::Value>::deserialize(deserializer)?;
        Ok(raw
            .into_iter()
            .filter_map(|(key, value)| parse(value).map(|parsed| (key, parsed)))
            .collect())
    }

    pub fn option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: serde::de::DeserializeOwned,
    {
        Ok(Option::<serde_json::Value>::deserialize(deserializer)?.and_then(parse))
    }

    pub fn or_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
    where
        D: Deserializer<'de>,
        T: serde::de::DeserializeOwned + Default,
    {
        Ok(parse(serde_json::Value::deserialize(deserializer)?).unwrap_or_default())
    }

    fn parse<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Option<T> {
        match serde_json::from_value(value) {
            Ok(parsed) => Some(parsed),
            Err(e) => {
                log::warn!("settings: dropping a value that no longer parses: {e}");
                None
            }
        }
    }
}

#[derive(PartialEq)]
struct Shards {
    core: Option<String>,
    look: Option<String>,
    windows: Option<String>,
    session: Option<String>,
    accounts: Option<String>,
}

/// Read one shard file, or its fields out of a pre-split `settings.json`. A
/// file that no longer parses resets to defaults and never falls through to
/// the legacy read, which would resurrect stale contents.
fn load_shard<T, F>(path: &Path, what: &str, legacy: &serde_json::Value, from_legacy: F) -> T
where
    T: Default + serde::de::DeserializeOwned,
    F: FnOnce(&serde_json::Value) -> T,
{
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            log::warn!("{what}: resetting {}: {e}", path.display());
            T::default()
        }),
        Err(_) => from_legacy(legacy),
    }
}

/// Read a shard out of a pre-split map, where every field kept its name. A
/// failure costs the whole shard, so it's logged.
fn from_legacy<T: Default + serde::de::DeserializeOwned>(value: &serde_json::Value) -> T {
    if value.is_null() {
        return T::default();
    }
    serde_json::from_value(value.clone()).unwrap_or_else(|e| {
        log::warn!("settings: reading the old file's contents: {e}");
        T::default()
    })
}

fn write_shard<T: Serialize>(
    path: PathBuf,
    what: &str,
    before: &Option<String>,
    after: &Option<String>,
    forced: bool,
    value: &T,
) {
    if forced || before != after || !path.exists() {
        write_json(&path, value, what);
    }
}

/// `settings.json`'s preferences plus the shards stored in their own files,
/// held together so callers go through one [`Settings::update`]. Unknown
/// fields drop on load and missing ones default, so files tolerate version
/// drift both ways.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Persisted to [`look_path`].
    #[serde(skip)]
    pub look: LookState,
    /// Persisted to [`windows_path`].
    #[serde(skip)]
    pub windows: WindowsState,
    /// Persisted to [`session_path`].
    #[serde(skip)]
    pub session: SessionState,
    /// Persisted to [`accounts_path`].
    #[serde(skip)]
    pub accounts: AccountsState,
    /// Read out of a pre-split file, so force a rewrite. A no-op edit would
    /// serialize to the same bytes and leave the stale keys, credentials
    /// included, on disk forever.
    #[serde(skip)]
    migrated: bool,
    pub library_roots: Vec<PathBuf>,
    /// Legacy single root, read once to seed `library_roots`, never written.
    #[serde(skip_serializing)]
    library_root: Option<PathBuf>,
    /// Globs the scan and watcher leave out, compiled by
    /// `rox_library::exclude`.
    pub library_exclude: Vec<String>,
    pub watch_library: bool,
    /// Rock and rock count as one value, shown under the majority casing.
    /// Flipping it reloads the projection.
    pub fold_case: bool,
    /// Commas and slashes split genre lists alongside the semicolon.
    /// Flipping it reloads the projection.
    pub split_genre_compounds: bool,
    /// Non-Latin names show their sort name as a reading, "秋ノ風 (Aki no
    /// kaze)".
    pub show_readings: bool,
    #[serde(deserialize_with = "lenient::or_default")]
    pub theme: Theme,
    /// A locale id from rox-i18n's registry. None follows the OS; an unknown
    /// id negotiates the same way instead of failing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// The app-wide rem in px. 16 is gpui's stock rem.
    pub app_font_size: f32,
    /// Gates the restore only; the last track is written either way.
    pub restore_last_track: bool,
    /// One switch for every scrobble destination; the connections stay
    /// either way.
    pub scrobbling: bool,
    /// The played fraction that scrobbles, shared by every destination.
    /// History's listen rule is fixed and doesn't read this.
    pub scrobble_threshold: f32,
    pub eq: EqSettings,
    /// Seconds. Zero keeps the gapless splice (ADR 19). Same-album
    /// boundaries never fade unless `crossfade_albums` is on.
    pub crossfade_secs: f32,
    /// The length the transport's toggle restores, since `crossfade_secs`
    /// stores off as zero. Never zero.
    pub crossfade_restore_secs: f32,
    pub crossfade_albums: bool,
    /// How far the comma and dot step keys move the playhead, in ms.
    pub step_ms: f32,
    /// How long a paused step plays, in ms. Separate from the step, since a
    /// 25 ms preview is inaudible.
    pub step_preview_ms: f32,
    pub replay_gain: ReplayGainSettings,
    pub output: OutputSettings,
    /// The icecast broadcast sink (ADR 22).
    pub broadcast: BroadcastSettings,
    pub capture: CaptureSettings,
    /// Seconds of a live stream kept in memory behind the playhead, which is
    /// what makes a station pausable. Held to [`clamp_live_buffer_secs`]:
    /// fifteen minutes of a 320 kbps stream is 36 MB.
    pub live_buffer_secs: u32,
    /// Closing the last window leaves the app resident in the tray (dock on
    /// macOS).
    pub quit_to_tray: bool,
    /// Whether the layout can be edited in place. A preference rather than
    /// part of the bundle, so applying a workspace leaves it alone.
    pub design_mode: bool,
    /// Reserve panel resizing for design mode. A preference, not part of the
    /// bundle.
    pub resize_lock: bool,
    /// Check GitHub for a newer release at launch, at most once a day.
    pub check_updates: bool,
    /// Consider release candidates. A candidate build is always opted in, so
    /// it learns about the release that closes its cycle.
    pub prerelease_updates: bool,
    /// Download and stage a newer release for the next start. Moot where the
    /// install can't update itself.
    pub download_updates: bool,
    /// Offer experimental panels. A layout already holding one restores it
    /// either way.
    pub experimental: bool,
    /// Whether anything of rox talks to AI tooling (ADR 22). Acoustic
    /// analysis never reads this.
    pub ai_enabled: bool,
    /// Whether MCP answers tool calls. The rox-mcp proxy checks it on every
    /// call, so a flip applies to the next tool use.
    pub mcp_enabled: bool,
    /// Whether the library may compute the acoustic vectors behind "more like
    /// this". Separate from the AI switches, since it costs decoding time.
    pub acoustic_analysis: bool,
    /// Whether the analysis pass follows the watcher. Off by default, like
    /// [`ReplayGainSettings::auto`].
    pub acoustic_auto: bool,
    /// The tempo pass behind the BPM column. Off, nothing measures and the
    /// column isn't offered.
    pub tempo_analysis: bool,
    pub tempo_auto: bool,
    /// Analysis workers, clamped to this machine's cores when a pass starts
    /// so a copied settings file can't oversubscribe it.
    pub acoustic_workers: usize,
    /// ReplayGain workers, by album. Each pass keeps its own count because
    /// their costs differ.
    pub replaygain_workers: usize,
    pub tempo_workers: usize,
    /// The catalog id the analysis pass runs and similarity reads. An unknown
    /// id or deleted weights fall back to the built-in extractor.
    pub acoustic_model: String,
    /// The downloadable model the ML Models page offers. Kept apart from
    /// `acoustic_model` so switching back from the built-in extractor
    /// remembers it.
    pub acoustic_ml_model: String,
    /// A weights file outside the catalog. Its id comes from the file's hash,
    /// so its vectors never land in another model's coordinates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acoustic_local_model: Option<LocalModel>,
    /// Read once when a pass starts.
    #[serde(deserialize_with = "lenient::or_default")]
    pub acoustic_save: AcousticSave,
    pub post_shader: PostShaderConfig,
    /// The Milkdrop visual behind the whole app. Machine settings, not the
    /// bundle, since it depends on preset packs on this disk.
    pub backdrop_visual: BackdropVisualConfig,
    pub milkdrop: MilkdropSettings,
    pub convert: ConvertSettings,
    /// Chords moved off their defaults, by command id; only overrides are
    /// written. An empty list means unbound, so never remove the key to
    /// unbind.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub keymap: BTreeMap<String, Vec<String>>,
}

/// The serde form of `rox_acoustic::Local`.
#[derive(Clone, Serialize, Deserialize)]
pub struct LocalModel {
    /// Absolute, and never copied: rox re-reads it whenever a pass starts.
    pub path: PathBuf,
    pub id: String,
    /// The [`file_stamp`] taken with the hash, checked before a pass so a
    /// rewritten checkpoint gets re-hashed. Zero reads as changed.
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub mtime: i64,
}

/// A file's size and mtime in unix seconds. A rewrite within the same second
/// as the hashed write goes unseen.
pub fn file_stamp(path: &Path) -> Option<(u64, i64)> {
    let meta = std::fs::metadata(path).ok().filter(|meta| meta.is_file())?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some((meta.len(), mtime))
}

/// `windows.json`: machine state, safe to delete.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowsState {
    #[serde(alias = "window", deserialize_with = "lenient::option")]
    pub main: Option<WindowState>,
    #[serde(deserialize_with = "lenient::option")]
    pub tag_editor: Option<TagEditorState>,
    #[serde(deserialize_with = "lenient::option")]
    pub rename_dialog: Option<RenameDialogState>,
    #[serde(alias = "stats_window", deserialize_with = "lenient::option")]
    pub stats: Option<StatsWindowState>,
    #[serde(deserialize_with = "lenient::option")]
    pub health: Option<HealthWindowState>,
    #[serde(deserialize_with = "lenient::option")]
    pub search: Option<SearchWindowState>,
    #[serde(alias = "settings_window", deserialize_with = "lenient::option")]
    pub settings: Option<LayoutSize>,
    #[serde(alias = "console_window", deserialize_with = "lenient::option")]
    pub console: Option<LayoutSize>,
    #[serde(deserialize_with = "lenient::option")]
    pub tasks: Option<LayoutSize>,
    #[serde(deserialize_with = "lenient::option")]
    pub convert_dialog: Option<LayoutSize>,
    #[serde(deserialize_with = "lenient::option")]
    pub bake_dialog: Option<LayoutSize>,
    #[serde(alias = "eq_window", deserialize_with = "lenient::option")]
    pub eq: Option<LayoutSize>,
    #[serde(deserialize_with = "lenient::option")]
    pub milkdrop_picker: Option<MilkdropPickerWindowState>,
    #[serde(deserialize_with = "lenient::option")]
    pub signals: Option<SignalsWindowState>,
    #[serde(alias = "panel_settings_window", deserialize_with = "lenient::option")]
    pub panel_settings: Option<LayoutSize>,
    /// The modal and popped-out queue's view. Raw JSON so the file stays
    /// readable when the queue's config schema moves.
    pub queue_view: Option<serde_json::Value>,
}

/// `session.json`: the volatile playback state, kept off the preferences file
/// so a volume nudge doesn't churn it. Safe to delete.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct SessionState {
    /// Linear, 0 to 2 like the engine's clamp.
    pub volume: f32,
    pub muted: bool,
    /// "off", "all", or "one". The engine's `LoopMode` stays serde-free, so
    /// convert through the accessors.
    pub loop_mode: String,
    /// Separate from [`Self::shuffle_mode`], so toggling keeps the picked
    /// mode.
    pub shuffle: bool,
    #[serde(deserialize_with = "lenient::or_default")]
    pub shuffle_mode: ShuffleMode,
    /// Which strategy refills the queue when it runs dry (ADR 17).
    pub continuation: continuation::Mode,
    /// A library track id, so a moved file still resolves.
    #[serde(deserialize_with = "lenient::option")]
    pub last_track: Option<LastTrack>,
    /// Preferred over [`SessionState::last_track`] when present.
    #[serde(deserialize_with = "lenient::option")]
    pub last_queue: Option<QueueState>,
    /// Unix seconds of the last full scan; launch rescans only when it's
    /// stale. Session state because it describes this machine's disk.
    pub last_scan: i64,
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient::option"
    )]
    pub update_cache: Option<UpdateCache>,
    /// The version whose menubar chip was dismissed; a newer one shows again.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient::option"
    )]
    pub update_dismissed: Option<String>,
    pub appimage_menu_declined: bool,
    /// Measured worker-seconds per track by model id, for pricing a pass
    /// before it runs. Per model and per machine, since both vary it by up
    /// to an order of magnitude.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub acoustic_pace: HashMap<String, f32>,
    #[serde(skip_serializing_if = "is_zero")]
    pub replaygain_pace: f32,
    #[serde(skip_serializing_if = "is_zero")]
    pub tempo_pace: f32,
    /// Worker-seconds per value, not per track.
    #[serde(skip_serializing_if = "is_zero")]
    pub romanize_pace: f32,
    /// Hex SHA-256 of the trimmed WGSL of every shader this machine agreed to
    /// run. Never written by an apply, and machine-local so a copied settings
    /// file can't carry someone else's trust decision.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub approved_shaders: BTreeSet<String>,
}

fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

/// `accounts.json`: session keys and API secrets, kept out of the settings
/// file people share. Not disposable.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountsState {
    /// The shared scrobble switch and threshold live on [`Settings`].
    pub lastfm: Lastfm,
    pub listenbrainz: ListenBrainz,
    /// Its own session: same protocol as Last.fm, different account.
    pub librefm: LibreFm,
    /// The online enrichment providers (ADR 14).
    pub providers: Providers,
    pub discord: DiscordSettings,
    pub subsonic_servers: Vec<SubsonicAccount>,
    /// Legacy single server, read once into `subsonic_servers`, never
    /// written.
    #[serde(skip_serializing)]
    subsonic: Option<SubsonicAccount>,
}

impl AccountsState {
    /// A list that's already there wins, since only this build writes one.
    fn fold_legacy_subsonic(&mut self) {
        let Some(legacy) = self.subsonic.take() else {
            return;
        };

        if self.subsonic_servers.is_empty() && !legacy.url.trim().is_empty() {
            self.subsonic_servers.push(legacy);
        }
    }
}

impl Default for SessionState {
    fn default() -> Self {
        SessionState {
            // Not derived: a zero volume would open a fresh install silent.
            volume: 1.0,
            muted: false,
            loop_mode: "off".into(),
            shuffle: false,
            shuffle_mode: ShuffleMode::Random,
            continuation: continuation::Mode::default(),
            last_track: None,
            last_queue: None,
            last_scan: 0,
            update_cache: None,
            update_dismissed: None,
            appimage_menu_declined: false,
            acoustic_pace: HashMap::new(),
            replaygain_pace: 0.0,
            tempo_pace: 0.0,
            romanize_pace: 0.0,
            approved_shaders: BTreeSet::new(),
        }
    }
}

impl SessionState {
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

/// The order shuffle puts the upcoming queue in. An unknown value falls back
/// to Random only through `lenient::or_default`, so any field holding one
/// needs that read or it fails its shard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShuffleMode {
    #[default]
    Random,
    /// Nearest first by acoustic vector to the track playing when engaged.
    Similar,
}

impl ShuffleMode {
    pub fn label(self) -> &'static str {
        match self {
            ShuffleMode::Random => "Random",
            ShuffleMode::Similar => "Similar",
        }
    }

    pub const ALL: [ShuffleMode; 2] = [ShuffleMode::Random, ShuffleMode::Similar];
}

/// Which user palette renders; System follows the OS live. Bundles hold no
/// theme, so applying a look never flips it.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Dark,
    Light,
    #[default]
    System,
}

static THEME: RwLock<Theme> = RwLock::new(Theme::Dark);

/// Cached because the platform read borrows the whole Wayland client, which
/// panics inside window construction or event dispatch. Window observers
/// keep it fresh.
static OS_APPEARANCE: RwLock<WindowAppearance> = RwLock::new(WindowAppearance::Light);

pub fn theme() -> Theme {
    *THEME.read().unwrap()
}

pub fn set_theme(theme: Theme, cx: &mut App) {
    *THEME.write().unwrap() = theme;
    palette::set_mode(resolve_theme(theme), cx);
}

/// Swap the interface language and repaint every window, since the locale
/// static is outside gpui's reactivity.
pub fn set_language(language: Option<&str>, cx: &mut App) {
    rox_i18n::set_locale(language);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

fn resolve_theme(theme: Theme) -> palette::Mode {
    match theme {
        Theme::Dark => palette::Mode::Dark,
        Theme::Light => palette::Mode::Light,
        Theme::System => match *OS_APPEARANCE.read().unwrap() {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => palette::Mode::Dark,
            WindowAppearance::Light | WindowAppearance::VibrantLight => palette::Mode::Light,
        },
    }
}

/// Call once at startup before [`set_theme`]: the only place the platform
/// read is safe, since the event loop isn't running yet.
pub fn seed_os_appearance(cx: &App) {
    *OS_APPEARANCE.write().unwrap() = cx.window_appearance();
}

/// Every window reports here; the mode setter dedupes repeats.
pub fn note_os_appearance(appearance: WindowAppearance, cx: &mut App) {
    *OS_APPEARANCE.write().unwrap() = appearance;
    if theme() == Theme::System {
        palette::set_mode(resolve_theme(Theme::System), cx);
    }
}

/// Five stars or a 0-10 number in half steps, both over the library's 0-100
/// value.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum RatingStyle {
    #[default]
    Stars,
    Numeric,
}

/// Holds the latest release rather than a yes/no, so an update clears a
/// cached "available" on its own.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateCache {
    pub checked_at: u64,
    /// The leading v stripped.
    pub latest: String,
    pub url: String,
}

static RATING_NUMERIC: AtomicBool = AtomicBool::new(false);

pub fn rating_style() -> RatingStyle {
    if RATING_NUMERIC.load(Ordering::Relaxed) {
        RatingStyle::Numeric
    } else {
        RatingStyle::Stars
    }
}

/// Repaints every window, since the static is outside gpui's reactivity. The
/// other live-flag setters below follow the same pattern.
pub fn set_rating_style(style: RatingStyle, cx: &mut App) {
    RATING_NUMERIC.store(style == RatingStyle::Numeric, Ordering::Relaxed);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

static RATING_DOTS: AtomicBool = AtomicBool::new(false);

pub fn rating_dots() -> bool {
    RATING_DOTS.load(Ordering::Relaxed)
}

pub fn set_rating_dots(on: bool, cx: &mut App) {
    RATING_DOTS.store(on, Ordering::Relaxed);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

static HIDE_MENUBAR: AtomicBool = AtomicBool::new(false);

pub fn hide_menubar() -> bool {
    HIDE_MENUBAR.load(Ordering::Relaxed)
}

pub fn set_hide_menubar(on: bool, cx: &mut App) {
    HIDE_MENUBAR.store(on, Ordering::Relaxed);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// Which of the menubar's status-side buttons draw.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct MenubarButtons {
    /// The tasks button.
    pub tasks: bool,
    /// The sleep timer button.
    pub sleep: bool,
    /// The rescan button.
    pub rescan: bool,
}

impl Default for MenubarButtons {
    fn default() -> Self {
        MenubarButtons {
            tasks: true,
            sleep: true,
            rescan: true,
        }
    }
}

impl MenubarButtons {
    const TASKS: u8 = 1;
    const SLEEP: u8 = 2;
    const RESCAN: u8 = 4;

    fn to_bits(self) -> u8 {
        ((self.tasks as u8) * Self::TASKS)
            | ((self.sleep as u8) * Self::SLEEP)
            | ((self.rescan as u8) * Self::RESCAN)
    }

    fn from_bits(bits: u8) -> Self {
        MenubarButtons {
            tasks: bits & Self::TASKS != 0,
            sleep: bits & Self::SLEEP != 0,
            rescan: bits & Self::RESCAN != 0,
        }
    }
}

static MENUBAR_BUTTONS: AtomicU8 =
    AtomicU8::new(MenubarButtons::TASKS | MenubarButtons::SLEEP | MenubarButtons::RESCAN);

pub fn menubar_buttons() -> MenubarButtons {
    MenubarButtons::from_bits(MENUBAR_BUTTONS.load(Ordering::Relaxed))
}

pub fn set_menubar_buttons(buttons: MenubarButtons, cx: &mut App) {
    MENUBAR_BUTTONS.store(buttons.to_bits(), Ordering::Relaxed);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// No repaint in the setter: flipping it reloads the projection, which
/// repaints everything.
static FOLD_CASE: AtomicBool = AtomicBool::new(false);

pub fn fold_case() -> bool {
    FOLD_CASE.load(Ordering::Relaxed)
}

pub fn set_fold_case(on: bool) {
    FOLD_CASE.store(on, Ordering::Relaxed);
}

static SHOW_READINGS: AtomicBool = AtomicBool::new(true);

pub fn show_readings() -> bool {
    SHOW_READINGS.load(Ordering::Relaxed)
}

pub fn set_show_readings(on: bool, cx: &mut App) {
    SHOW_READINGS.store(on, Ordering::Relaxed);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// Child windows only follow this with [`bare_child_windows`] on.
static OS_DECORATIONS: AtomicBool = AtomicBool::new(true);

pub fn os_decorations() -> bool {
    OS_DECORATIONS.load(Ordering::Relaxed)
}

pub fn window_decorations() -> WindowDecorations {
    if os_decorations() {
        WindowDecorations::Server
    } else {
        WindowDecorations::Client
    }
}

/// Open windows are renegotiated by the caller (`workspace::apply_decorations`).
pub fn set_os_decorations(on: bool) {
    OS_DECORATIONS.store(on, Ordering::Relaxed);
}

/// Whether child windows go bare with the main ones. Opt-in, since they
/// only have the fallback titlebar to stand in for the OS chrome.
static BARE_CHILD_WINDOWS: AtomicBool = AtomicBool::new(false);

pub fn bare_child_windows() -> bool {
    BARE_CHILD_WINDOWS.load(Ordering::Relaxed)
}

pub fn set_bare_child_windows(on: bool) {
    BARE_CHILD_WINDOWS.store(on, Ordering::Relaxed);
}

pub fn child_window_decorations() -> WindowDecorations {
    if !os_decorations() && bare_child_windows() {
        WindowDecorations::Client
    } else {
        WindowDecorations::Server
    }
}

/// Whether bare child windows draw the fallback titlebar. Off leaves them
/// no chrome at all; Close Window still reaches them from the keyboard.
static CHILD_TITLEBAR: AtomicBool = AtomicBool::new(true);

pub fn child_titlebar() -> bool {
    CHILD_TITLEBAR.load(Ordering::Relaxed)
}

pub fn set_child_titlebar(on: bool) {
    CHILD_TITLEBAR.store(on, Ordering::Relaxed);
}

/// Rox's own window buttons: flat icons or macOS traffic lights.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ChromeStyle {
    #[default]
    Icons,
    Traffic,
}

/// Which end of the fallback titlebar the buttons sit at.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ChromeSide {
    Left,
    #[default]
    Right,
}

static CHROME_STYLE: AtomicU8 = AtomicU8::new(ChromeStyle::Icons as u8);
static CHROME_SIDE: AtomicU8 = AtomicU8::new(ChromeSide::Right as u8);

pub fn chrome_style() -> ChromeStyle {
    match CHROME_STYLE.load(Ordering::Relaxed) {
        1 => ChromeStyle::Traffic,
        _ => ChromeStyle::Icons,
    }
}

pub fn set_chrome_style(style: ChromeStyle) {
    CHROME_STYLE.store(style as u8, Ordering::Relaxed);
}

pub fn chrome_side() -> ChromeSide {
    match CHROME_SIDE.load(Ordering::Relaxed) {
        0 => ChromeSide::Left,
        _ => ChromeSide::Right,
    }
}

pub fn set_chrome_side(side: ChromeSide) {
    CHROME_SIDE.store(side as u8, Ordering::Relaxed);
}

/// Only applied on Windows; elsewhere a borderless window's edges already
/// do nothing.
static RESIZE_BORDER: AtomicBool = AtomicBool::new(true);

pub fn resize_border() -> bool {
    RESIZE_BORDER.load(Ordering::Relaxed)
}

/// The caller pushes it to open windows (`workspace::apply_resize_border`).
pub fn set_resize_border(on: bool) {
    RESIZE_BORDER.store(on, Ordering::Relaxed);
}

/// The seams, design-mode, and resize-lock flags live in the dock crate,
/// which reads them per frame. These wrappers keep the settings surface in
/// one place.
pub fn seams() -> bool {
    rox_dock::resizable::seams()
}

pub fn set_seams(on: bool, cx: &mut App) {
    rox_dock::resizable::set_seams(on);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

pub fn design_mode() -> bool {
    rox_dock::design_mode()
}

pub fn set_design_mode(on: bool, cx: &mut App) {
    rox_dock::set_design_mode(on);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

pub fn resize_lock() -> bool {
    rox_dock::resize_lock()
}

pub fn set_resize_lock(on: bool, cx: &mut App) {
    rox_dock::set_resize_lock(on);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

static QUIT_TO_TRAY: AtomicBool = AtomicBool::new(false);

pub fn quit_to_tray() -> bool {
    QUIT_TO_TRAY.load(Ordering::Relaxed)
}

/// The caller reconciles the tray icon (`tray::sync`).
pub fn set_quit_to_tray(on: bool) {
    QUIT_TO_TRAY.store(on, Ordering::Relaxed);
}

static EXPERIMENTAL: AtomicBool = AtomicBool::new(false);

pub fn experimental() -> bool {
    EXPERIMENTAL.load(Ordering::Relaxed)
}

pub fn set_experimental(on: bool, cx: &mut App) {
    EXPERIMENTAL.store(on, Ordering::Relaxed);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

static ACOUSTIC_ANALYSIS: AtomicBool = AtomicBool::new(false);

pub fn acoustic_analysis() -> bool {
    ACOUSTIC_ANALYSIS.load(Ordering::Relaxed)
}

pub fn set_acoustic_analysis(on: bool, cx: &mut App) {
    ACOUSTIC_ANALYSIS.store(on, Ordering::Relaxed);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

static TEMPO_ANALYSIS: AtomicBool = AtomicBool::new(false);

pub fn tempo_analysis() -> bool {
    TEMPO_ANALYSIS.load(Ordering::Relaxed)
}

pub fn set_tempo_analysis(on: bool, cx: &mut App) {
    TEMPO_ANALYSIS.store(on, Ordering::Relaxed);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// What the library's Gain column draws. The engine levels by the player's
/// own copy, not this.
static GAIN_MODE: AtomicU8 = AtomicU8::new(0);

pub fn gain_mode() -> GainModeSetting {
    match GAIN_MODE.load(Ordering::Relaxed) {
        1 => GainModeSetting::Track,
        2 => GainModeSetting::Album,
        _ => GainModeSetting::Off,
    }
}

pub fn set_gain_mode(mode: GainModeSetting, cx: &mut App) {
    GAIN_MODE.store(
        match mode {
            GainModeSetting::Off => 0,
            GainModeSetting::Track => 1,
            GainModeSetting::Album => 2,
        },
        Ordering::Relaxed,
    );
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// Whether the model in use has vectors, as opposed to the switch merely
/// permitting the pass. Published by whoever learns the answer.
static ACOUSTIC_DESCRIBED: AtomicBool = AtomicBool::new(false);

/// What every surface that offers Similar is gated on.
pub fn similarity_ready() -> bool {
    acoustic_analysis() && ACOUSTIC_DESCRIBED.load(Ordering::Relaxed)
}

pub fn set_acoustic_described(described: bool, cx: &mut App) {
    if ACOUSTIC_DESCRIBED.swap(described, Ordering::Relaxed) == described {
        return;
    }
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// None follows the platform default.
static APP_FONT: RwLock<Option<SharedString>> = RwLock::new(None);

pub fn app_font() -> Option<SharedString> {
    APP_FONT.read().unwrap().clone()
}

pub fn set_app_font(font: Option<String>, cx: &mut App) {
    *APP_FONT.write().unwrap() = font.map(SharedString::from);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

pub const DEFAULT_CROSSFADE_SECS: f32 = 4.0;

pub const DEFAULT_STEP_MS: f32 = 25.0;

/// The ceiling meets the seek keys' five seconds.
pub const STEP_MS_MIN: f32 = 1.0;
pub const STEP_MS_MAX: f32 = 5000.0;

pub const DEFAULT_STEP_PREVIEW_MS: f32 = 100.0;

pub const STEP_PREVIEW_MS_MIN: f32 = 10.0;
pub const STEP_PREVIEW_MS_MAX: f32 = STEP_MS_MAX;

/// Fifteen minutes, about 36 MB at 320 kbps. Also the mark the Capture
/// section warns under, since capture slices songs out of this buffer.
pub const DEFAULT_LIVE_BUFFER_SECS: u32 = 900;

/// The ceiling is twelve hours, about 1.7 GB at 320 kbps.
/// [`rox_playback::memory::live_buffer_cap`] caps the bytes under the whole
/// band, since the bitrate isn't known when this is set.
pub const LIVE_BUFFER_SECS_MIN: u32 = 30;
pub const LIVE_BUFFER_SECS_MAX: u32 = 43200;

pub fn clamp_live_buffer_secs(secs: u32) -> u32 {
    secs.clamp(LIVE_BUFFER_SECS_MIN, LIVE_BUFFER_SECS_MAX)
}

/// The frame knobs' ceilings in px, shared by the app and per-panel sliders.
pub const MARGIN_MAX: f32 = 24.0;
pub const PADDING_MAX: f32 = 24.0;
pub const ROUNDING_MAX: f32 = 24.0;
pub const BORDER_MAX: f32 = 6.0;

fn clamp_knob(value: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, max)
    } else {
        0.0
    }
}

/// ADR 13's frame knobs in px, the defaults every panel's `PanelTheme` can
/// override.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct Frame {
    pub margin: Sides,
    pub padding: Sides,
    pub rounding: f32,
    pub border: Sides,
}

impl Frame {
    pub const DEFAULT: Frame = Frame {
        margin: Sides::ZERO,
        padding: Sides::ZERO,
        rounding: 0.0,
        border: Sides::ZERO,
    };

    /// Held to the ceilings, non-finite reset to zero, for hand-edited files.
    pub fn clamped(self) -> Frame {
        Frame {
            margin: self.margin.clamped(MARGIN_MAX),
            padding: self.padding.clamped(PADDING_MAX),
            rounding: clamp_knob(self.rounding, ROUNDING_MAX),
            border: self.border.clamped(BORDER_MAX),
        }
    }
}

impl Default for Frame {
    fn default() -> Self {
        Frame::DEFAULT
    }
}

static FRAME: RwLock<Frame> = RwLock::new(Frame::DEFAULT);

pub fn app_frame() -> Frame {
    *FRAME.read().unwrap()
}

pub fn set_app_frame(frame: Frame, cx: &mut App) {
    *FRAME.write().unwrap() = frame.clamped();
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// How the quick-play modal draws its result list.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct QuickPlayConfig {
    /// Show a cover thumbnail at the left of each result.
    pub show_cover: bool,
    /// Show the artist and album line under each title.
    pub show_subtitle: bool,
    /// Show each result's duration on the right.
    pub show_duration: bool,
    /// Give each result row more height.
    pub comfortable: bool,
}

impl Default for QuickPlayConfig {
    fn default() -> Self {
        QuickPlayConfig {
            show_cover: false,
            show_subtitle: true,
            show_duration: true,
            comfortable: false,
        }
    }
}

/// One image a shader samples via `// @asset name: file`, stored in the
/// bundle as the encoded file in base64.
///
/// Assets never gate: approval is over code, and an image can't run anything
/// (ADR 23).
#[derive(Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct ShaderAsset {
    /// The flat filename the shader declares it under, also used by eject.
    pub file: String,
    /// The encoded image file in base64.
    pub data: String,
}

impl ShaderAsset {
    pub fn from_bytes(file: impl Into<String>, bytes: &[u8]) -> Self {
        ShaderAsset {
            file: file.into(),
            data: BASE64.encode(bytes),
        }
    }

    pub fn decode(&self) -> Result<Vec<u8>, String> {
        BASE64
            .decode(self.data.as_bytes())
            .map_err(|err| err.to_string())
    }
}

/// One shader in a workspace's pool. The inline source is canonical; the
/// path is a local hot-reload bookmark that export scrubs.
#[derive(Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct NamedShader {
    /// Unique within a pool; the last entry wins on a repeat.
    pub name: String,
    /// The fragment stage: a `fs_user(uv)` definition and whatever it calls.
    pub source: String,
    /// The ejected working copy, for hot reload. None in exported bundles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// The images the source declares with `// @asset`.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient::vec"
    )]
    pub assets: Vec<ShaderAsset>,
}

/// The whole-window post-process shader.
#[derive(Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct PostShaderConfig {
    /// Whether the pass runs at all.
    pub enabled: bool,
    /// The WGSL fragment source file, absolute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// The fragment stage inline, so it travels in a bundle. Empty falls back
    /// to reading `path` at startup.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// A pool entry by name, which wins over the inline source. A name that
    /// resolves to nothing runs nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether child windows get the shader too. The confirm dialog stays
    /// bare either way, so a hostile shader can't hide the way out.
    pub all_windows: bool,
    /// Signal routes into the sixteen slots. Empty fills signal i into slot
    /// i; any route replaces that fill, so unrouted slots read zero.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<Route>,
    /// Hand-set slot values, read where nothing is routed. A route on the
    /// same slot wins.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub manual: Vec<(u8, f32)>,
    /// Keep drawing while the audio is silent. The clock only advances with
    /// the signal feed either way.
    pub run_when_idle: bool,
}

impl PostShaderConfig {
    /// False for an untouched default, which keeps it out of an export.
    pub fn configured(&self) -> bool {
        self.enabled || !self.source.is_empty() || self.name.is_some() || self.path.is_some()
    }
}

/// Cached because the gate runs in a paint path. Every write goes through
/// [`approve_shader`] to keep the cache and file in step.
static APPROVED_SHADERS: LazyLock<RwLock<BTreeSet<String>>> =
    LazyLock::new(|| RwLock::new(Settings::load().session.approved_shaders));

/// Hashes of the shaders this build ships, trusted by construction. Never
/// persisted, since the set changes with the build.
static SHIPPED_SHADERS: LazyLock<RwLock<BTreeSet<String>>> =
    LazyLock::new(|| RwLock::new(BTreeSet::new()));

pub fn trust_shipped(fingerprints: impl IntoIterator<Item = String>) {
    SHIPPED_SHADERS.write().unwrap().extend(fingerprints);
}

pub fn shader_approved(fingerprint: &str) -> bool {
    APPROVED_SHADERS.read().unwrap().contains(fingerprint)
        || SHIPPED_SHADERS.read().unwrap().contains(fingerprint)
}

/// Cache only, for tests. Everything else approves through
/// [`approve_shader`], which persists.
pub fn note_approved(fingerprint: &str) -> bool {
    APPROVED_SHADERS
        .write()
        .unwrap()
        .insert(fingerprint.to_string())
}

pub fn approve_shader(fingerprint: &str) {
    if !note_approved(fingerprint) {
        return;
    }
    let fingerprint = fingerprint.to_string();
    Settings::update(move |s| {
        s.session.approved_shaders.insert(fingerprint);
    });
}

/// Test cleanup; nothing in the UI revokes an approval.
pub fn forget_approved(fingerprint: &str) {
    APPROVED_SHADERS.write().unwrap().remove(fingerprint);
}

/// Cached like [`APPROVED_SHADERS`]; writes go through [`set_shader_pool`].
static SHADER_POOL: LazyLock<RwLock<Vec<NamedShader>>> =
    LazyLock::new(|| RwLock::new(Settings::load().look.bundle.shaders));

/// Bumped on every replacement, so a cached resolution checks staleness with
/// one load instead of diffing WGSL every frame.
static SHADER_POOL_REV: AtomicU64 = AtomicU64::new(0);

/// Cloned out so a render never holds the lock and blocks a shader edit.
pub fn shader_pool() -> Vec<NamedShader> {
    SHADER_POOL.read().unwrap().clone()
}

pub fn shader_pool_get(name: &str) -> Option<NamedShader> {
    SHADER_POOL
        .read()
        .unwrap()
        .iter()
        .find(|shader| shader.name == name)
        .cloned()
}

pub fn set_shader_pool(shaders: Vec<NamedShader>) {
    note_shader_pool(shaders.clone());
    Settings::update(move |s| {
        s.look.bundle.shaders = shaders;
    });
}

/// Cache only, for tests and for a workspace apply that already wrote the
/// bundle.
pub fn note_shader_pool(shaders: Vec<NamedShader>) {
    *SHADER_POOL.write().unwrap() = shaders;
    SHADER_POOL_REV.fetch_add(1, Ordering::Relaxed);
}

pub fn shader_pool_rev() -> u64 {
    SHADER_POOL_REV.load(Ordering::Relaxed)
}

/// Cached like the pool, since the workspace root reads it every render.
static BACKDROP_SHADER: LazyLock<RwLock<Option<PostShaderConfig>>> =
    LazyLock::new(|| RwLock::new(Settings::load().look.bundle.backdrop_shader.clone()));

/// None for a bare art wash.
pub fn backdrop_shader() -> Option<PostShaderConfig> {
    BACKDROP_SHADER.read().unwrap().clone()
}

/// Cache only, like [`note_shader_pool`].
pub fn note_backdrop_shader(config: Option<PostShaderConfig>) {
    *BACKDROP_SHADER.write().unwrap() = config;
}

/// The Milkdrop backdrop's live, merged config. The skipped fields belong to
/// [`MilkdropLook`] and are filled from the look on load and on apply, so
/// the machine file never carries a second copy.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BackdropVisualConfig {
    #[serde(skip)]
    pub enabled: bool,
    #[serde(skip)]
    pub strength: f32,
    /// Fraction of the window's device pixels. Defaults well below 1, since
    /// half the side is a quarter of the readback under a heavy blur.
    pub scale: f32,
    /// Every frame is a readback, so 30 costs half of 60.
    pub fps: u32,
    /// How readily projectM calls something a beat, 0 to 5.
    pub beat_sensitivity: f32,
    pub hard_cuts: bool,
    /// Rotation folder under a scan root, forward slashes. `favorites_only`
    /// wins over it while on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation_folder: Option<String>,
    #[serde(skip)]
    pub flip_horizontal: bool,
    #[serde(skip)]
    pub flip_vertical: bool,
    /// With nothing starred the worker falls back to everything.
    pub favorites_only: bool,
    /// Stay on the current preset: no timed switch, no cut on a beat.
    pub locked: bool,
    pub duration_secs: f64,
    #[serde(skip)]
    pub color: MilkdropColor,
    /// On fades to the cover over `fade_secs` on pause or stop. Off holds
    /// the last frame.
    pub fade: bool,
    pub fade_secs: f32,
    /// Written on a pick and, while locked, on every switch. An unlocked
    /// rotation isn't worth a settings write.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<PathBuf>,
}

/// How a Milkdrop frame's colours are treated before they reach the screen.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MilkdropColor {
    /// The preset's own colours, whatever the theme.
    Preset,
    /// The preset's own colours, lightness inverted on the light theme.
    #[default]
    Theme,
    /// Lightness mapped onto a ramp from the theme's background to its accent.
    Palette,
    /// A ramp through the playing cover's dominant colour, or the accent
    /// while nothing plays.
    Cover,
}

pub const BACKDROP_VISUAL_DURATION: f64 = 30.0;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MilkdropSettings {
    /// One app-wide list, in starring order.
    pub favorites: Vec<PathBuf>,
    /// Preset folders beyond `milkdrop_dir()/presets`, which is always
    /// scanned. Machine-local, so a layout never carries a path.
    pub roots: Vec<PathBuf>,
}

pub const BACKDROP_VISUAL_STRENGTH: f32 = 0.35;

pub const BACKDROP_VISUAL_SCALE: f32 = 0.5;

pub const BACKDROP_VISUAL_FADE_SECS: f32 = 0.7;
pub const BACKDROP_VISUAL_FADE_MAX: f32 = 5.0;

/// Half the panel's sixty, since the backdrop runs the whole time the app
/// plays.
pub const BACKDROP_VISUAL_FPS: u32 = 30;

pub const BACKDROP_VISUAL_FPS_MIN: u32 = 10;
pub const BACKDROP_VISUAL_FPS_MAX: u32 = 240;

impl Default for BackdropVisualConfig {
    fn default() -> Self {
        BackdropVisualConfig {
            enabled: false,
            strength: BACKDROP_VISUAL_STRENGTH,
            scale: BACKDROP_VISUAL_SCALE,
            fps: BACKDROP_VISUAL_FPS,
            beat_sensitivity: 1.0,
            hard_cuts: true,
            rotation_folder: None,
            flip_horizontal: false,
            flip_vertical: false,
            favorites_only: false,
            locked: true,
            duration_secs: BACKDROP_VISUAL_DURATION,
            color: MilkdropColor::default(),
            fade: true,
            fade_secs: BACKDROP_VISUAL_FADE_SECS,
            preset: None,
        }
    }
}

impl BackdropVisualConfig {
    pub fn with_look(mut self, look: &MilkdropLook) -> BackdropVisualConfig {
        self.enabled = look.enabled;
        self.strength = look.strength;
        self.color = look.color;
        self.flip_horizontal = look.flip_horizontal;
        self.flip_vertical = look.flip_vertical;
        self.clamped()
    }

    pub fn look(&self) -> MilkdropLook {
        MilkdropLook {
            enabled: self.enabled,
            strength: self.strength,
            color: self.color,
            flip_horizontal: self.flip_horizontal,
            flip_vertical: self.flip_vertical,
        }
    }

    /// Hand-edited numbers pulled into range, since they feed render math and
    /// framebuffer sizes directly.
    fn clamped(mut self) -> BackdropVisualConfig {
        self.strength = if self.strength.is_finite() {
            self.strength.clamp(0.0, 1.0)
        } else {
            BACKDROP_VISUAL_STRENGTH
        };
        self.scale = if self.scale.is_finite() {
            self.scale.clamp(0.1, 1.0)
        } else {
            BACKDROP_VISUAL_SCALE
        };
        // Zero or NaN in projectM's timer is a preset switch every frame.
        self.duration_secs = if self.duration_secs.is_finite() {
            self.duration_secs.clamp(1.0, 120.0)
        } else {
            BACKDROP_VISUAL_DURATION
        };
        self.beat_sensitivity = if self.beat_sensitivity.is_finite() {
            self.beat_sensitivity.clamp(0.0, 5.0)
        } else {
            1.0
        };
        // Feeds a Duration, which panics on a negative or NaN.
        self.fade_secs = if self.fade_secs.is_finite() {
            self.fade_secs.clamp(0.0, BACKDROP_VISUAL_FADE_MAX)
        } else {
            BACKDROP_VISUAL_FADE_SECS
        };
        // Zero is a worker that never renders; older files read as zero.
        self.fps = if self.fps == 0 {
            BACKDROP_VISUAL_FPS
        } else {
            self.fps
                .clamp(BACKDROP_VISUAL_FPS_MIN, BACKDROP_VISUAL_FPS_MAX)
        };
        self
    }
}

/// Cached because every window's backdrop layer reads it per frame.
static BACKDROP_VISUAL: LazyLock<RwLock<BackdropVisualConfig>> = LazyLock::new(|| {
    let settings = Settings::load();
    RwLock::new(
        settings
            .backdrop_visual
            .clone()
            .with_look(&settings.look.bundle.appearance.milkdrop),
    )
});

pub fn backdrop_visual() -> BackdropVisualConfig {
    BACKDROP_VISUAL.read().unwrap().clone()
}

/// Cache only, so a slider drag doesn't write settings per pixel.
pub fn note_backdrop_visual(config: BackdropVisualConfig) {
    *BACKDROP_VISUAL.write().unwrap() = config.clamped();
}

/// Take a look's Milkdrop fields into the live config, keeping the
/// machine's own.
pub fn set_backdrop_visual_look(look: &MilkdropLook) {
    let mut cache = BACKDROP_VISUAL.write().unwrap();
    *cache = cache.clone().with_look(look);
}

/// The Milkdrop lists are cached, since surfaces check them per frame.
static MILKDROP_FAVORITES: LazyLock<RwLock<Vec<PathBuf>>> =
    LazyLock::new(|| RwLock::new(Settings::load().milkdrop.favorites.clone()));

static MILKDROP_ROOTS: LazyLock<RwLock<Vec<PathBuf>>> =
    LazyLock::new(|| RwLock::new(Settings::load().milkdrop.roots.clone()));

/// Bumped on every edit to either list, so surfaces skip the list compare.
static MILKDROP_GEN: AtomicU64 = AtomicU64::new(0);

pub fn milkdrop_favorites() -> Vec<PathBuf> {
    MILKDROP_FAVORITES.read().unwrap().clone()
}

pub fn milkdrop_roots() -> Vec<PathBuf> {
    MILKDROP_ROOTS.read().unwrap().clone()
}

pub fn milkdrop_scan_roots() -> Vec<PathBuf> {
    let mut roots = vec![milkdrop_dir().join("presets")];
    roots.extend(milkdrop_roots());
    roots
}

/// Returns whether the list changed.
pub fn set_milkdrop_roots(roots: Vec<PathBuf>) -> bool {
    {
        let mut held = MILKDROP_ROOTS.write().unwrap();
        if *held == roots {
            return false;
        }
        *held = roots.clone();
    }
    MILKDROP_GEN.fetch_add(1, Ordering::AcqRel);
    Settings::update(move |s| s.milkdrop.roots = roots);
    true
}

pub fn milkdrop_gen() -> u64 {
    MILKDROP_GEN.load(Ordering::Acquire)
}

pub fn is_milkdrop_favorite(path: &Path) -> bool {
    MILKDROP_FAVORITES
        .read()
        .unwrap()
        .iter()
        .any(|favorite| favorite == path)
}

/// Returns whether the list changed; a no-op costs no write.
pub fn set_milkdrop_favorite(path: &Path, on: bool) -> bool {
    let changed = {
        let mut favorites = MILKDROP_FAVORITES.write().unwrap();
        let held = favorites.iter().position(|favorite| favorite == path);
        match (held, on) {
            (None, true) => {
                favorites.push(path.to_path_buf());
                true
            }
            (Some(index), false) => {
                favorites.remove(index);
                true
            }
            _ => false,
        }
    };
    if changed {
        MILKDROP_GEN.fetch_add(1, Ordering::AcqRel);
        let favorites = milkdrop_favorites();
        Settings::update(move |s| s.milkdrop.favorites = favorites);
    }
    changed
}

/// Last.fm binds a session to the api key that authorized it, so only a
/// build signing with that key can use it.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LastfmSession {
    /// Empty means this api key has no session: never connected,
    /// disconnected, or refused.
    pub key: String,
    pub username: String,
}

impl LastfmSession {
    fn connected(&self) -> bool {
        !self.key.is_empty()
    }
}

/// The key and secret override the build's own api identity
/// (`lastfm::keys`), for builds that ship none.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Lastfm {
    pub api_key: String,
    pub api_secret: String,
    /// Sessions by the api key that minted each, since nix, release, and
    /// local builds sign differently but share this file.
    ///
    /// The empty key holds a pre-split file's session. The first build it
    /// works for claims it ([`Self::attribute`]); every other build files
    /// its own refusal.
    pub sessions: BTreeMap<String, LastfmSession>,
    /// The newest imported scrobble's timestamp, by lowercased username.
    /// Per account, never global: a second account would otherwise import
    /// nothing, since its history predates the first account's bound.
    pub imported: BTreeMap<String, i64>,
    /// Legacy session, read once into the unattributed slot, never written.
    #[serde(skip_serializing)]
    session_key: String,
    #[serde(skip_serializing)]
    username: String,
    /// Legacy, read once to seed [`Settings::scrobbling`], never written.
    #[serde(skip_serializing)]
    scrobbling: Option<bool>,
    /// Whether the heart also sends a Last.fm love. Off by default, and it
    /// never pushes favourites that predate turning it on.
    pub love_favourites: bool,
    /// Legacy, read once to seed [`Settings::scrobble_threshold`], never
    /// written.
    #[serde(skip_serializing)]
    threshold: Option<f32>,
}

pub fn clamp_threshold(threshold: f32) -> f32 {
    if threshold.is_finite() {
        threshold.clamp(0.1, 1.0)
    } else {
        0.5
    }
}

const UNATTRIBUTED: &str = "";

impl Lastfm {
    /// This key's own session, or the unattributed one while this key has
    /// never tried. A present but empty entry is a recorded refusal, so a
    /// launch never retries a session it was told isn't its own.
    pub fn session(&self, api_key: &str) -> Option<&LastfmSession> {
        if api_key.is_empty() {
            return None;
        }
        match self.sessions.get(api_key) {
            Some(session) => session.connected().then_some(session),
            None => self.sessions.get(UNATTRIBUTED).filter(|s| s.connected()),
        }
    }

    pub fn username(&self, api_key: &str) -> &str {
        self.session(api_key).map_or("", |s| s.username.as_str())
    }

    pub fn connected_elsewhere(&self, api_key: &str) -> bool {
        self.sessions
            .iter()
            .any(|(key, session)| key != api_key && session.connected())
    }

    pub fn connect(&mut self, api_key: &str, key: String, username: String) {
        self.sessions
            .insert(api_key.to_string(), LastfmSession { key, username });
    }

    /// Disconnect, or record a refusal. The entry stays empty rather than
    /// going away, since an absent key would retry the unattributed session.
    /// An empty key returns early: its entry is the unattributed slot.
    pub fn clear_session(&mut self, api_key: &str) {
        if api_key.is_empty() {
            return;
        }
        self.sessions
            .insert(api_key.to_string(), LastfmSession::default());
    }

    /// Claim the unattributed session for the key that just used it
    /// successfully. True when the caller should persist.
    pub fn attribute(&mut self, api_key: &str) -> bool {
        if api_key.is_empty() || self.sessions.contains_key(api_key) {
            return false;
        }
        let Some(session) = self.sessions.remove(UNATTRIBUTED) else {
            return false;
        };
        self.sessions.insert(api_key.to_string(), session);
        true
    }

    pub fn imported_through(&self, user: &str) -> Option<i64> {
        self.imported.get(&user.to_lowercase()).copied()
    }

    /// Never moves the bound backwards, or a run stopped partway would skip
    /// everything above it.
    pub fn note_import(&mut self, user: &str, through: i64) {
        let slot = self.imported.entry(user.to_lowercase()).or_default();
        *slot = (*slot).max(through);
    }

    /// Call when the listens are cleared, or a re-import comes back empty.
    pub fn forget_imports(&mut self) {
        self.imported.clear();
    }

    fn fold_legacy_session(&mut self) {
        let (key, username) = (
            std::mem::take(&mut self.session_key),
            std::mem::take(&mut self.username),
        );
        if key.is_empty() || !self.sessions.is_empty() {
            return;
        }
        self.sessions
            .insert(UNATTRIBUTED.to_string(), LastfmSession { key, username });
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ListenBrainz {
    /// The user token from listenbrainz.org/settings. Empty means not
    /// connected.
    pub token: String,
    pub username: Option<String>,
}

/// Libre.fm takes any api pair, so there's no per-key session map.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LibreFm {
    /// Empty means not connected.
    pub session_key: String,
    pub username: String,
}

/// A Subsonic or OpenSubsonic library source. The password is stored in the
/// clear because the protocol derives a per-request token from it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SubsonicAccount {
    /// Off leaves the rows in the library.
    pub enabled: bool,
    /// Empty lets the address's host stand in.
    pub name: String,
    /// Base URL with scheme, no `/rest` on the end.
    pub url: String,
    pub user: String,
    pub password: String,
    /// Unix seconds, for display only; syncs are never scheduled off it.
    pub last_sync: i64,
}

impl SubsonicAccount {
    /// The typed name, or the address's host.
    pub fn label(&self) -> String {
        let name = self.name.trim();
        if !name.is_empty() {
            return name.to_string();
        }

        let url = self.url.trim();
        let rest = url.split_once("://").map_or(url, |(_, rest)| rest);

        rest.split(['/', '?', '#'])
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

/// Where a fetched lyrics sheet saves.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LyricsSave {
    Tag,
    Sidecar,
    /// [`lyrics_dir`], so fetches never touch the library.
    #[default]
    Store,
}

/// Hashed-name `.lrc` files. The first save creates it.
pub fn lyrics_dir() -> PathBuf {
    data_dir().join("lyrics")
}

/// Fetched bios and images, plus per-track caches under `tracks/` and
/// `releases/`. The first fetch creates it.
pub fn artists_dir() -> PathBuf {
    data_dir().join("artists")
}

/// The online enrichment providers (ADR 14). On by default is fine because
/// they only fetch on a user action.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Providers {
    pub lrclib: bool,
    #[serde(deserialize_with = "lenient::or_default")]
    pub lyrics_save: LyricsSave,
    pub musicbrainz: bool,
    pub itunes: bool,
    pub deezer: bool,
    pub lastfm_art: bool,
    /// Artist bios from Last.fm, with Deezer and theaudiodb images.
    pub artist: bool,
    pub acoustid: bool,
    /// A user's own AcoustID key. Empty uses the build's key, if it has one.
    pub acoustid_key: String,
}

impl Default for Providers {
    fn default() -> Self {
        Providers {
            lrclib: true,
            lyrics_save: LyricsSave::default(),
            musicbrainz: true,
            itunes: true,
            deezer: true,
            lastfm_art: true,
            artist: true,
            acoustid: true,
            acoustid_key: String::new(),
        }
    }
}

/// One text file per saved curve. The first save creates it.
pub fn eq_presets_dir() -> PathBuf {
    data_dir().join("eq").join("presets")
}

/// The equalizer's saved curve (ADR 19). The live values are atomics
/// (`rox_services::player::set_eq_gain`); this seeds them and gets flushed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqSettings {
    pub enabled: bool,
    /// A list so a file with a different band count still loads.
    pub gains: Vec<f32>,
    /// Band centers in Hz. Empty loads the ISO octaves.
    #[serde(default)]
    pub freqs: Vec<f32>,
    /// Band widths. Empty loads one octave.
    #[serde(default)]
    pub qs: Vec<f32>,
    #[serde(default, deserialize_with = "lenient::or_default")]
    pub analyzer: AnalyzerStyle,
    /// In samples. Snapped to a power of two on read, so a hand-edited value
    /// can't panic the window.
    pub fft_size: usize,
}

/// How the equalizer draws the music behind its curve.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnalyzerStyle {
    #[default]
    Wave,
    Bars,
    Off,
}

impl Default for EqSettings {
    fn default() -> Self {
        EqSettings {
            enabled: false,
            gains: vec![0.0; rox_playback::eq::BANDS],
            freqs: rox_playback::eq::BAND_HZ.to_vec(),
            qs: vec![rox_playback::eq::Q_DEFAULT; rox_playback::eq::BANDS],
            analyzer: AnalyzerStyle::default(),
            // Long, or the bottom two octaves fall into a handful of bins.
            fft_size: 8192,
        }
    }
}

/// How tagged loudness is levelled (ADR 19). Off by default to keep the
/// bit-perfect claim.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplayGainSettings {
    #[serde(deserialize_with = "lenient::or_default")]
    pub mode: GainModeSetting,
    /// Added to every tagged gain, in dB.
    pub preamp_db: f32,
    /// What an untagged file plays at, in dB.
    pub fallback_db: f32,
    /// Read by the measurement pass when it starts, never by the engine.
    #[serde(deserialize_with = "lenient::or_default")]
    pub save: ReplayGainSave,
    /// Whether the measurement pass follows the watcher (ADR 19). Off by
    /// default, since tags mode rewrites files.
    pub auto: bool,
}

/// Where a measured ReplayGain is written (ADR 19).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplayGainSave {
    /// Never rewrites a file or bumps an mtime.
    #[default]
    Database,
    /// Rewrites the audio files so other players read the same numbers.
    Tags,
}

/// Where an acoustic vector is written. Unlike [`ReplayGainSave`], the
/// database row is written either way, since similarity queries read the
/// table; tags mode adds a copy that outlives the database.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AcousticSave {
    /// Never rewrites a file or bumps an mtime.
    #[default]
    Database,
    /// Also the file's tags, for MP3 and FLAC only.
    Tags,
}

/// The persisted spelling of [`rox_playback::gain::GainMode`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GainModeSetting {
    #[default]
    Off,
    Track,
    Album,
}

impl ReplayGainSettings {
    pub fn rule(&self) -> rox_playback::gain::GainRule {
        use rox_playback::gain::GainMode;
        rox_playback::gain::GainRule {
            mode: match self.mode {
                GainModeSetting::Off => GainMode::Off,
                GainModeSetting::Track => GainMode::Track,
                GainModeSetting::Album => GainMode::Album,
            },
            preamp_db: self.preamp_db,
            fallback_db: self.fallback_db,
        }
    }
}

/// How samples reach the device (ADR 19). A request: what the hardware
/// accepted lives on the running session.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputSettings {
    /// A failed exclusive claim falls back to shared, never to silence.
    pub exclusive: bool,
    /// By cpal name. None or an unknown name follows the system default.
    pub device: Option<String>,
    /// By ALSA name, kept apart because cpal and ALSA names don't cross.
    pub exclusive_device: Option<String>,
    /// None follows each file's rate (ADR 19). Pinning trades bit-perfect
    /// for no reopen gap at rate changes.
    #[serde(default)]
    pub rate: Option<u32>,
    /// `f32`, `s32`, or `s16`. None takes the widest the device offers.
    #[serde(default)]
    pub format: Option<String>,
    /// In ms. Lower crackles on a loaded machine.
    #[serde(default)]
    pub period_ms: Option<f64>,
}

/// The icecast broadcast sink (ADR 22). The source password stays in
/// `settings.json`, since it's shared-secret plumbing rather than an
/// account credential.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BroadcastSettings {
    pub enabled: bool,
    /// No scheme; the source protocol runs over a plain socket.
    pub host: String,
    pub port: u16,
    /// A leading slash is optional.
    pub mount: String,
    pub user: String,
    pub password: String,
    pub name: String,
    /// In kbps, snapped to the nearest LAME step.
    pub bitrate: u32,
}

impl Default for BroadcastSettings {
    fn default() -> Self {
        BroadcastSettings {
            enabled: false,
            host: String::new(),
            // icecast's stock port and source user.
            port: 8000,
            mount: "/rox".into(),
            user: "source".into(),
            password: String::new(),
            name: String::new(),
            bitrate: 192,
        }
    }
}

/// Saving songs off a live stream, cut at the in-band title changes and
/// written without re-encoding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaptureSettings {
    pub enabled: bool,
    /// Its own folder rather than a library root, since station boundaries
    /// are approximate.
    pub folder: PathBuf,
    /// A renamer pattern; "/" makes a subfolder. Read through
    /// [`CaptureSettings::parsed_pattern`], never straight.
    pub pattern: String,
    /// The album tag as a pattern. Empty by default, so the station name
    /// doesn't pollute album views; it always goes in the comment.
    pub album: String,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        CaptureSettings {
            enabled: false,
            folder: default_capture_folder(),
            pattern: DEFAULT_CAPTURE_PATTERN.to_string(),
            album: String::new(),
        }
    }
}

pub const DEFAULT_CAPTURE_PATTERN: &str = "%station%/%artist% - %title%";

impl CaptureSettings {
    /// Falls back to the default pattern, so a hand-edited or outdated one
    /// can't stop captures.
    pub fn parsed_pattern<F: PatternField>(&self) -> Pattern<F> {
        if let Ok(pattern) = pattern::parse(&self.pattern) {
            return pattern;
        }

        log::warn!(
            "capture: {:?} is not a pattern, using the default",
            self.pattern
        );

        pattern::parse(DEFAULT_CAPTURE_PATTERN).expect("the default capture pattern parses")
    }
}

pub fn default_capture_folder() -> PathBuf {
    dirs::audio_dir()
        .map(|dir| dir.join("rox Captures"))
        .unwrap_or_else(|| data_dir().join("captures"))
}

/// Presence card patterns in the renamer's grammar.
pub const DEFAULT_PRESENCE_FIRST_LINE: &str = "%artist% - %title%";

pub const DEFAULT_PRESENCE_SECOND_LINE: &str = "%album%";

pub const DEFAULT_PRESENCE_HOVER: &str = "%format%";

/// Which card line Discord repeats in the member list. Discord only picks
/// from the card's own fields; a client can't override the app name.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiscordStatusLine {
    App,
    #[default]
    First,
    Second,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscordSettings {
    pub enabled: bool,
    pub show_lastfm_button: bool,
    pub show_youtube_button: bool,
    /// Empty leaves the line off; an unparseable pattern renders the default.
    pub first_line: String,
    pub second_line: String,
    pub hover_line: String,
    #[serde(deserialize_with = "lenient::or_default")]
    pub status_line: DiscordStatusLine,
}

impl Default for DiscordSettings {
    fn default() -> Self {
        DiscordSettings {
            enabled: false,
            show_lastfm_button: true,
            show_youtube_button: true,
            first_line: DEFAULT_PRESENCE_FIRST_LINE.to_string(),
            second_line: DEFAULT_PRESENCE_SECOND_LINE.to_string(),
            hover_line: DEFAULT_PRESENCE_HOVER.to_string(),
            status_line: DiscordStatusLine::First,
        }
    }
}

/// A named dock layout preset. The dump stays raw JSON so the file loads
/// when the layout schema moves.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NamedLayout {
    pub name: String,
    pub dump: serde_json::Value,
    /// The window size the preset restores to. None keeps the current size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<LayoutSize>,
}

/// A single configured panel saved under a name. Raw JSON keeps rox-core off
/// the dock crate.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PanelPreset {
    pub name: String,
    /// The dock's `PanelState` as JSON.
    pub panel: serde_json::Value,
}

impl PanelPreset {
    /// The panel's registry name, without deserializing the dump.
    pub fn panel_name(&self) -> Option<&str> {
        self.panel.get("panel_name")?.as_str()
    }
}

/// A window size in logical pixels.
#[derive(Clone, Copy, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct LayoutSize {
    pub width: f32,
    pub height: f32,
}

/// A layout's unsaved working state, kept apart from the saved preset.
#[derive(Clone, Serialize, Deserialize)]
pub struct LayoutEdit {
    pub dump: serde_json::Value,
    /// None falls back to the preset's saved size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<LayoutSize>,
}

/// Independent of the dock layout version the dumps carry.
pub const WORKSPACE_VERSION: u32 = 1;

/// A shareable workspace: layouts, palettes, pools, and appearance. Machine
/// and account state stays out, so a bundle travels as pure look.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct WorkspaceBundle {
    /// Format version; a reader refuses a bundle from a newer format.
    pub version: u32,
    /// The bundle's name. Empty falls back to the file stem.
    pub name: String,
    /// The layout presets, each a named dock dump.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient::vec"
    )]
    pub layouts: Vec<NamedLayout>,
    /// The panel presets. In the bundle because they can name pool shaders.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient::vec"
    )]
    pub panel_presets: Vec<PanelPreset>,
    /// The mini-player button's primary layout, by preset name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_layout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mini_layout: Option<String>,
    /// Role name to `#rrggbb`. Empty means the designed defaults.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub palette_dark: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub palette_light: BTreeMap<String, String>,
    /// The signal pool the looks route from. An apply replaces it wholesale.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient::vec"
    )]
    pub signals: Vec<Signal>,
    /// The named WGSL the looks point into. An apply replaces it wholesale.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient::vec"
    )]
    pub shaders: Vec<NamedShader>,
    /// Who made this workspace and what it is.
    #[serde(skip_serializing_if = "WorkspaceMeta::is_empty")]
    pub meta: WorkspaceMeta,
    /// The whole-window shader. None applies as disabled, never as "leave
    /// what's there".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_shader: Option<PostShaderConfig>,
    /// The shader over the backdrop and under the panels. None means a bare
    /// backdrop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backdrop_shader: Option<PostShaderConfig>,
    /// The appearance knobs.
    pub appearance: AppearanceBundle,
}

impl Default for WorkspaceBundle {
    fn default() -> Self {
        WorkspaceBundle {
            version: WORKSPACE_VERSION,
            name: String::new(),
            layouts: Vec::new(),
            panel_presets: Vec::new(),
            primary_layout: None,
            mini_layout: None,
            palette_dark: BTreeMap::new(),
            palette_light: BTreeMap::new(),
            signals: Vec::new(),
            shaders: Vec::new(),
            meta: WorkspaceMeta::default(),
            post_shader: None,
            backdrop_shader: None,
            appearance: AppearanceBundle::default(),
        }
    }
}

/// The JSON Schema for saved workspace files (ADR 22), current write shape
/// only. A test holds `assets/workspace.schema.json` to this output.
pub fn workspace_schema() -> serde_json::Value {
    let mut schema = serde_json::to_value(schemars::schema_for!(WorkspaceBundle))
        .expect("schema serializes: it is built from plain maps");
    if let Some(root) = schema.as_object_mut() {
        // The derive marks nothing required since every field defaults on
        // read, but the writer always produces these three.
        root.insert(
            "required".into(),
            serde_json::json!(["version", "name", "appearance"]),
        );
    }
    schema
}

/// The card on a workspace. Free text throughout; empty means unset.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct WorkspaceMeta {
    /// Who made it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub author: String,
    /// What the look is going for.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Where to find it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub website: String,
    /// The author's own version string, unrelated to the file format's.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// The terms it's shared under.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub license: String,
    /// When it was first exported, ISO `YYYY-MM-DD`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub created: String,
    /// When it was last exported, same shape.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub updated: String,
}

impl WorkspaceMeta {
    pub fn is_empty(&self) -> bool {
        self.author.is_empty()
            && self.description.is_empty()
            && self.website.is_empty()
            && self.version.is_empty()
            && self.license.is_empty()
            && self.created.is_empty()
            && self.updated.is_empty()
    }

    /// `updated` always moves; `created` is written once.
    pub fn stamp(&mut self, today: &str) {
        if self.created.is_empty() {
            self.created = today.to_string();
        }
        self.updated = today.to_string();
    }

    /// Fill this card's gaps from the one being saved over, so an overwrite
    /// never wipes a filled-in card. `created` always comes back; `updated`
    /// is left alone.
    pub fn carry_forward(&mut self, prior: &WorkspaceMeta) {
        for (mine, theirs) in [
            (&mut self.author, &prior.author),
            (&mut self.description, &prior.description),
            (&mut self.website, &prior.website),
            (&mut self.version, &prior.version),
            (&mut self.license, &prior.license),
        ] {
            if mine.is_empty() {
                mine.clone_from(theirs);
            }
        }
        if !prior.created.is_empty() {
            self.created.clone_from(&prior.created);
        }
    }
}

fn utc_today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// The appearance knobs a workspace carries. The theme pick and font size
/// stay out: they're the user's own choices.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct AppearanceBundle {
    /// How opaque the app's surfaces read, 0 to 1 (ADR 10).
    pub surface_opacity: f32,
    /// How strongly the backdrop shows behind the surfaces, 0 to 1.
    pub backdrop_strength: f32,
    /// Whether child windows paint the cover backdrop too.
    pub backdrop_all_windows: bool,
    /// The frame defaults every panel inherits, in px.
    pub frame: Frame,
    /// Whether the 1px seams between panels paint. Off still resizes.
    pub seams: bool,
    /// Whether the playing track's art re-tints the palette (ADR 10).
    pub art_theming: bool,
    /// Keep song theming from swapping between light and dark.
    pub keep_theme: bool,
    /// The app-wide font family. None follows the platform default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_font: Option<String>,
    /// How ratings read and click.
    #[serde(deserialize_with = "lenient::or_default")]
    pub rating_style: RatingStyle,
    /// Whether unfilled star slots draw a faint dot.
    pub rating_dots: bool,
    /// The quick-play modal's appearance.
    pub quick_play: QuickPlayConfig,
    /// The Milkdrop backdrop's look. Per-machine state lives in
    /// [`BackdropVisualConfig`].
    pub milkdrop: MilkdropLook,
    /// Hide the menubar except while alt is held or a menu is open.
    pub hide_menubar: bool,
    /// Which of the bar's status-side buttons show.
    pub menubar_buttons: MenubarButtons,
    /// Whether the main windows get the OS's own titlebar and borders.
    pub os_decorations: bool,
    /// Whether child windows go bare too while `os_decorations` is off.
    pub bare_child_windows: bool,
    /// Whether those bare child windows draw the fallback titlebar.
    pub child_titlebar: bool,
    /// How the fallback titlebar's buttons draw.
    #[serde(deserialize_with = "lenient::or_default")]
    pub chrome_style: ChromeStyle,
    /// Which end of the fallback titlebar the buttons sit at.
    #[serde(deserialize_with = "lenient::or_default")]
    pub chrome_side: ChromeSide,
    /// Whether bare windows resize from their edges. Windows only.
    pub resize_border: bool,
}

impl Default for AppearanceBundle {
    fn default() -> Self {
        AppearanceBundle {
            surface_opacity: 1.0,
            backdrop_strength: 1.0,
            backdrop_all_windows: true,
            frame: Frame::DEFAULT,
            seams: true,
            art_theming: false,
            keep_theme: false,
            app_font: None,
            rating_style: RatingStyle::default(),
            rating_dots: false,
            quick_play: QuickPlayConfig::default(),
            milkdrop: MilkdropLook::default(),
            hide_menubar: false,
            menubar_buttons: MenubarButtons::default(),
            os_decorations: true,
            bare_child_windows: false,
            child_titlebar: true,
            chrome_style: ChromeStyle::default(),
            chrome_side: ChromeSide::default(),
            resize_border: true,
        }
    }
}

/// The Milkdrop backdrop's share of a look.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct MilkdropLook {
    /// Whether the visual runs at all.
    pub enabled: bool,
    /// The pass's alpha over the blurred cover, 0 to 1.
    pub strength: f32,
    /// How the frame's colours meet the theme.
    pub color: MilkdropColor,
    /// Mirror the frame left to right.
    pub flip_horizontal: bool,
    /// Mirror the frame top to bottom.
    pub flip_vertical: bool,
}

impl Default for MilkdropLook {
    fn default() -> Self {
        MilkdropLook {
            enabled: false,
            strength: BACKDROP_VISUAL_STRENGTH,
            color: MilkdropColor::default(),
            flip_horizontal: false,
            flip_vertical: false,
        }
    }
}

/// `workspace.json`: the live bundle plus working state that never travels.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LookState {
    /// Its `name` is the workspace it was applied from, if any.
    pub bundle: WorkspaceBundle,
    /// Raw dock JSON, so the file stays readable when the layout schema
    /// moves.
    pub layout: Option<serde_json::Value>,
    /// The named preset in front of you. None for an unnamed arrangement.
    pub active_layout: Option<String>,
    /// Unsaved tweaks for the layouts not in front of you, by name. The
    /// active one lives in `layout`; a save folds its copy in and clears it.
    #[serde(
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "lenient::map"
    )]
    pub layout_edits: BTreeMap<String, LayoutEdit>,
}

impl LookState {
    /// Rebuild the look from a pre-split `settings.json`. Field names never
    /// changed, so both halves deserialize straight out of the flat map.
    fn from_legacy(value: &serde_json::Value) -> LookState {
        let mut bundle: WorkspaceBundle = serde_json::from_value(value.clone()).unwrap_or_default();
        // The appearance knobs were flat siblings, so they take their own pass.
        bundle.appearance = serde_json::from_value(value.clone()).unwrap_or_default();
        bundle.name = String::new();
        bundle.version = WORKSPACE_VERSION;
        LookState {
            bundle,
            layout: value.get("layout").cloned().filter(|v| !v.is_null()),
            active_layout: value
                .get("active_layout")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            layout_edits: value
                .get("layout_edits")
                .cloned()
                .and_then(|v| serde_json::from_value(v).ok())
                .unwrap_or_default(),
        }
    }
}

/// The Shader panel's dock name, spelled here because the dump walks work on
/// raw JSON below the crate that defines it.
const SHADER_PANEL: &str = "shader";

/// Take the shader file bookmarks out of a dock dump.
fn scrub_dump_paths(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            // Any panel's surface shader, flattened onto its config.
            if let Some(serde_json::Value::Object(shader)) = map.get_mut("shader") {
                shader.remove("path");
            }
            // The Shader panel, whose config is the shader.
            if map.get("panel_name").and_then(|name| name.as_str()) == Some(SHADER_PANEL)
                && let Some(serde_json::Value::Object(config)) =
                    map.get_mut("info").and_then(|info| info.get_mut("panel"))
            {
                config.remove("path");
            }
            for child in map.values_mut() {
                scrub_dump_paths(child);
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(scrub_dump_paths),
        _ => {}
    }
}

/// Every shader source a dock dump holds.
///
/// One of four walks over the same two shapes ([`scrub_dump_paths`],
/// [`dump_wears_shader`], [`strip_dump_shaders`]), split by `&mut`. Whatever
/// gets added to one belongs in all of them.
pub fn dump_shader_sources(value: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    collect_dump_shader_sources(value, &mut out);
    out
}

/// Whether anything in a dock dump would paint a shader, trusted or not. A
/// pool name counts the same as inline text.
pub fn dump_wears_shader(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => {
            // Any panel's surface shader, flattened onto its config.
            if let Some(serde_json::Value::Object(shader)) = map.get("shader") {
                let on = shader
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                if on && (has_text(shader, "name") || has_text(shader, "source")) {
                    return true;
                }
            }
            // The Shader panel counts unless emptied or switched off. A
            // config saying nothing runs the shipped example.
            if map.get("panel_name").and_then(|name| name.as_str()) == Some(SHADER_PANEL) {
                let config = map.get("info").and_then(|info| info.get("panel"));
                let quiet = match config {
                    Some(serde_json::Value::Object(config)) => {
                        // Blank source is emptied; a missing one runs the
                        // default.
                        let emptied = !has_text(config, "name")
                            && config.contains_key("source")
                            && !has_text(config, "source");
                        let off = config.get("enabled").and_then(|v| v.as_bool()) == Some(false);
                        emptied || off
                    }
                    _ => false,
                };
                if !quiet {
                    return true;
                }
            }
            map.values().any(dump_wears_shader)
        }
        serde_json::Value::Array(items) => items.iter().any(dump_wears_shader),
        _ => false,
    }
}

fn has_text(map: &serde_json::Map<String, serde_json::Value>, key: &str) -> bool {
    map.get(key)
        .and_then(|v| v.as_str())
        .is_some_and(|text| !text.trim().is_empty())
}

/// Switch every shader in a dock dump off, for an apply without shaders.
/// Never delete them instead: the Shader panel would lose its way back to
/// the shader the look came with.
pub fn strip_dump_shaders(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Object(shader)) = map.get_mut("shader") {
                shader.insert("enabled".into(), serde_json::Value::Bool(false));
            }
            if map.get("panel_name").and_then(|name| name.as_str()) == Some(SHADER_PANEL) {
                // With no config it still runs the shipped example, so the
                // switch is written regardless.
                let info = map
                    .entry("info")
                    .or_insert_with(|| serde_json::json!({}))
                    .as_object_mut();
                if let Some(info) = info {
                    let config = info
                        .entry("panel")
                        .or_insert_with(|| serde_json::json!({}))
                        .as_object_mut();
                    if let Some(config) = config {
                        config.insert("enabled".into(), serde_json::Value::Bool(false));
                    }
                }
            }
            for child in map.values_mut() {
                strip_dump_shaders(child);
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(strip_dump_shaders),
        _ => {}
    }
}

fn collect_dump_shader_sources(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            // Any panel's surface shader, flattened onto its config.
            if let Some(serde_json::Value::Object(shader)) = map.get("shader")
                && let Some(source) = shader.get("source").and_then(|s| s.as_str())
            {
                out.push(source.to_string());
            }
            // The Shader panel, whose config is the shader.
            if map.get("panel_name").and_then(|name| name.as_str()) == Some(SHADER_PANEL)
                && let Some(serde_json::Value::Object(config)) =
                    map.get("info").and_then(|info| info.get("panel"))
                && let Some(source) = config.get("source").and_then(|s| s.as_str())
            {
                out.push(source.to_string());
            }
            for child in map.values() {
                collect_dump_shader_sources(child, out);
            }
        }
        serde_json::Value::Array(items) => items
            .iter()
            .for_each(|item| collect_dump_shader_sources(item, out)),
        _ => {}
    }
}

impl WorkspaceBundle {
    /// Snapshot the shareable state into a named bundle. The live dock folds
    /// into this bundle's copy of the active layout ("Untitled" if unnamed),
    /// never into the live look's saved presets.
    pub fn from_settings(name: String, s: &Settings) -> WorkspaceBundle {
        let mut bundle = s.look.bundle.clone();
        bundle.version = WORKSPACE_VERSION;
        bundle.name = name;
        if let Some(dump) = s.look.layout.clone() {
            let active = s
                .look
                .active_layout
                .clone()
                .unwrap_or_else(|| "Untitled".to_string());
            let size = s.windows.main.as_ref().map(|w| LayoutSize {
                width: w.width,
                height: w.height,
            });
            if let Some(existing) = bundle.layouts.iter_mut().find(|l| l.name == active) {
                existing.dump = dump;
                existing.size = size;
            } else {
                bundle.layouts.push(NamedLayout {
                    name: active.clone(),
                    dump,
                    size,
                });
            }
            if bundle.primary_layout.is_none() {
                bundle.primary_layout = Some(active);
            }
        }
        // The screen shader lives in machine settings, so copy it in before
        // the passes that make it travel.
        if s.post_shader.configured() {
            bundle.post_shader = Some(s.post_shader.clone());
        }
        bundle.inline_post_shader();
        bundle.scrub_paths();
        bundle.meta.stamp(&utc_today());
        bundle
    }

    /// Pull path-only shaders' files inline so they travel. Best effort: an
    /// unreadable file leaves the source empty.
    pub fn inline_post_shader(&mut self) {
        for shader in [self.post_shader.as_mut(), self.backdrop_shader.as_mut()]
            .into_iter()
            .flatten()
        {
            if !shader.source.is_empty() {
                continue;
            }
            if let Some(path) = shader.path.as_ref()
                && let Ok(source) = std::fs::read_to_string(path)
            {
                shader.source = source;
            }
        }
    }

    /// Drop every shader file bookmark on the way out, so an import never
    /// aims a hot reload at someone else's file. Targets the two shader
    /// shapes by name: stripping every `path` key would take a folder
    /// panel's root with it.
    pub fn scrub_paths(&mut self) {
        for shader in &mut self.shaders {
            shader.path = None;
        }
        if let Some(post) = self.post_shader.as_mut() {
            post.path = None;
        }
        if let Some(backdrop) = self.backdrop_shader.as_mut() {
            backdrop.path = None;
        }
        for layout in &mut self.layouts {
            scrub_dump_paths(&mut layout.dump);
        }
    }

    /// The apply's persistence half. The layout swap and the live statics
    /// are the caller's, since they need the workspace and an `App`.
    pub fn apply_to(self, s: &mut Settings) {
        // Old working copies would shadow the incoming layouts.
        s.look.layout_edits.clear();
        s.look.bundle = self;
    }
}

/// The single-track fallback for files that predate [`QueueState`].
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LastTrack {
    pub id: i64,
    /// The cue track number, 0 for a plain file.
    pub sub: u16,
    pub position_secs: f64,
}

/// The play queue at close. An entry whose file has left the library drops
/// out on restore, and the cursor shifts to stay on the playing track.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct QueueState {
    /// History and upcoming both, in engine order.
    pub entries: Vec<QueuedTrack>,
    pub cursor: usize,
    pub position_secs: f64,
}

/// `explicit` marks a hand-queued entry, the only kind the queue panel lists.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct QueuedTrack {
    pub id: i64,
    /// The cue track number, stored because the restore can run before the
    /// projection is up. Defaulted per field, since the struct isn't.
    #[serde(default)]
    pub sub: u16,
    pub explicit: bool,
}

/// The tag editor's remembered shape; the last window to close wins.
///
/// A field column shows unless it's in `hidden`, so new fields appear on
/// their own. An opt-in column (extra tags, sort names) shows only when it's
/// in `shown`. `tag_columns` is keyed rather than positional because a tag's
/// place changes with the selection. `replace_ignore_case` is stored
/// inverted so older files read as exact match.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TagEditorState {
    pub width: f32,
    pub height: f32,
    pub columns: Vec<f32>,
    pub hidden: Vec<String>,
    pub shown: Vec<String>,
    pub tag_columns: BTreeMap<String, f32>,
    pub sort_fields: bool,
    pub pattern: String,
    pub replace_regex: bool,
    pub replace_ignore_case: bool,
}

/// `patterns` holds the last applied patterns, newest first.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RenameDialogState {
    pub width: f32,
    pub height: f32,
    pub patterns: Vec<String>,
}

/// What the convert dialog opens on. `preset` is a `convert::Preset` key or
/// "custom"; an unknown key falls back to the default. Empty `ffmpeg` means
/// the one on PATH.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ConvertSettings {
    pub preset: String,
    pub destination: Option<PathBuf>,
    pub pattern: String,
    /// A bare extension ("ogg"), read only for the custom preset.
    pub custom_ext: String,
    /// Split on whitespace, so there's no quoting.
    pub custom_args: String,
    pub mirror: bool,
    pub ffmpeg: String,
}

/// `range` is "all", "year", or "month"; an unknown key reads as all time.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StatsWindowState {
    pub width: f32,
    pub height: f32,
    pub range: String,
}

#[derive(Clone, Copy, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HealthWindowState {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SearchWindowState {
    pub width: f32,
    pub height: f32,
}

/// `about` is whether the explainer is unfolded.
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct SignalsWindowState {
    pub width: f32,
    pub height: f32,
    pub about: bool,
}

impl Default for SignalsWindowState {
    fn default() -> Self {
        SignalsWindowState {
            width: 0.,
            height: 0.,
            about: true,
        }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct MilkdropPickerWindowState {
    pub width: f32,
    pub height: f32,
    pub favorites_only: bool,
    pub nested: bool,
}

impl Default for MilkdropPickerWindowState {
    fn default() -> Self {
        MilkdropPickerWindowState {
            width: 0.,
            height: 0.,
            favorites_only: false,
            nested: true,
        }
    }
}

/// When maximized, the frame is the restore size.
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

/// Seed the shared threshold from the Last.fm account's legacy copy, only
/// while the settings file has never written it. `core` is the parsed file,
/// the only place that says whether the knob was written or defaulted.
fn seed_threshold(settings: &mut Settings, core: &serde_json::Value) {
    let legacy = settings.accounts.lastfm.threshold.take();
    if core.get("scrobble_threshold").is_some() {
        return;
    }
    if let Some(threshold) = legacy {
        settings.scrobble_threshold = threshold;
    }
}

/// The scrobbling switch, seeded the same way.
fn seed_scrobbling(settings: &mut Settings, core: &serde_json::Value) {
    let legacy = settings.accounts.lastfm.scrobbling.take();
    if core.get("scrobbling").is_some() {
        return;
    }
    if let Some(scrobbling) = legacy {
        settings.scrobbling = scrobbling;
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            look: LookState::default(),
            migrated: false,
            windows: WindowsState::default(),
            session: SessionState::default(),
            accounts: AccountsState::default(),
            library_roots: Vec::new(),
            library_root: None,
            library_exclude: Vec::new(),
            watch_library: true,
            fold_case: false,
            split_genre_compounds: true,
            show_readings: true,
            theme: Theme::default(),
            language: None,
            app_font_size: palette::FONT_SIZE_DEFAULT,
            restore_last_track: true,
            scrobbling: true,
            scrobble_threshold: 0.5,
            eq: EqSettings::default(),
            crossfade_secs: 0.0,
            crossfade_restore_secs: DEFAULT_CROSSFADE_SECS,
            step_ms: DEFAULT_STEP_MS,
            step_preview_ms: DEFAULT_STEP_PREVIEW_MS,
            crossfade_albums: false,
            replay_gain: ReplayGainSettings::default(),
            output: OutputSettings::default(),
            broadcast: BroadcastSettings::default(),
            capture: CaptureSettings::default(),
            live_buffer_secs: DEFAULT_LIVE_BUFFER_SECS,
            quit_to_tray: false,
            design_mode: true,
            resize_lock: false,
            check_updates: true,
            prerelease_updates: false,
            download_updates: false,
            experimental: false,
            ai_enabled: false,
            mcp_enabled: false,
            acoustic_analysis: false,
            acoustic_auto: false,
            tempo_analysis: false,
            tempo_auto: false,
            acoustic_workers: acoustic::DEFAULT_WORKERS,
            replaygain_workers: acoustic::DEFAULT_WORKERS,
            tempo_workers: acoustic::DEFAULT_WORKERS,
            acoustic_model: acoustic::MODEL.to_string(),
            acoustic_ml_model: acoustic::PANNS_CNN10.to_string(),
            acoustic_local_model: None,
            acoustic_save: AcousticSave::default(),
            post_shader: PostShaderConfig::default(),
            backdrop_visual: BackdropVisualConfig::default(),
            milkdrop: MilkdropSettings::default(),
            convert: ConvertSettings::default(),
            keymap: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// A missing or corrupt file resets to defaults rather than blocking start.
    pub fn load() -> Settings {
        let path = settings_path();
        // Parsed to a Value first, since the pre-split migration reads the
        // shards back out of this same map.
        let raw = std::fs::read_to_string(&path).ok();
        let value: serde_json::Value = match raw.as_deref() {
            Some(text) => serde_json::from_str(text).unwrap_or_else(|e| {
                log::warn!("settings: resetting {}: {e}", path.display());
                serde_json::Value::Null
            }),
            None => serde_json::Value::Null,
        };
        let mut settings: Settings = if value.is_null() {
            Settings::default()
        } else {
            serde_json::from_value(value.clone()).unwrap_or_else(|e| {
                log::warn!("settings: resetting {}: {e}", path.display());
                Settings::default()
            })
        };
        // Back up a pre-split file and drain its workspaces before the shards
        // read out of it.
        settings.migrated = raw.is_some() && Self::shard_missing();
        if let Some(text) = raw.as_deref()
            && settings.migrated
        {
            Self::migrate_split(&value, text);
        }
        settings.look = load_shard(&look_path(), "look", &value, LookState::from_legacy);
        settings.windows = load_shard(&windows_path(), "windows", &value, from_legacy);
        settings.session = load_shard(&session_path(), "session", &value, from_legacy);
        settings.accounts = load_shard(&accounts_path(), "accounts", &value, from_legacy);
        // Hand-edited values feed the engine, render math, and window bounds
        // directly, so everything below gets clamped.
        settings.session.volume = if settings.session.volume.is_finite() {
            settings.session.volume.clamp(0.0, 2.0)
        } else {
            1.0
        };
        let appearance = &mut settings.look.bundle.appearance;
        for scalar in [
            &mut appearance.surface_opacity,
            &mut appearance.backdrop_strength,
        ] {
            *scalar = if scalar.is_finite() {
                scalar.clamp(0.0, 1.0)
            } else {
                1.0
            };
        }
        appearance.frame = appearance.frame.clamped();
        // `with_look` clamps too.
        settings.backdrop_visual = settings
            .backdrop_visual
            .clone()
            .with_look(&settings.look.bundle.appearance.milkdrop);
        seed_threshold(&mut settings, &value);
        seed_scrobbling(&mut settings, &value);
        settings.scrobble_threshold = clamp_threshold(settings.scrobble_threshold);
        settings.accounts.lastfm.fold_legacy_session();
        settings.accounts.fold_legacy_subsonic();
        // Negative origins are real on multi-monitor setups, so only the
        // size gets floored.
        let bad_frame = settings
            .windows
            .main
            .as_ref()
            .is_some_and(|w| [w.x, w.y, w.width, w.height].iter().any(|v| !v.is_finite()));
        if bad_frame {
            settings.windows.main = None;
        } else if let Some(w) = settings.windows.main.as_mut() {
            w.width = w.width.max(f32::from(MIN_WINDOW_SIZE.width));
            w.height = w.height.max(f32::from(MIN_WINDOW_SIZE.height));
        }
        if settings.library_roots.is_empty()
            && let Some(root) = settings.library_root.take()
        {
            settings.library_roots.push(root);
        }
        settings
    }

    /// A missing shard means the settings file still holds the pre-split
    /// shape.
    fn shard_missing() -> bool {
        [look_path(), windows_path(), session_path(), accounts_path()]
            .iter()
            .any(|path| !path.exists())
    }

    /// Back up the single-file format and write out its workspaces. Once per
    /// process, and the drain skips workspaces already on disk, so a crash
    /// before the first save can't duplicate one.
    fn migrate_split(value: &serde_json::Value, raw: &str) {
        // Checked before the one-shot guard, so a load that beats the sink
        // into place doesn't burn the move.
        let Some(migrate) = WORKSPACE_MIGRATOR.get() else {
            return;
        };
        static DONE: AtomicBool = AtomicBool::new(false);
        if DONE.swap(true, Ordering::Relaxed) {
            return;
        }
        let backup = settings_path().with_extension("json.bak-presplit");
        if !backup.exists()
            && let Err(e) = std::fs::write(&backup, raw)
        {
            log::warn!("settings: backing up to {}: {e}", backup.display());
        }
        let saved: Vec<WorkspaceBundle> = value
            .get("workspaces")
            .cloned()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        if saved.is_empty() {
            return;
        }
        log::info!("settings: moving {} workspaces to files", saved.len());
        for bundle in saved {
            migrate(bundle);
        }
    }

    /// Reload, apply, and write back, so one writer's save never reverts
    /// another's fields.
    pub fn update(f: impl FnOnce(&mut Settings)) {
        // Serialize the read-modify-write, or a background writer and the
        // UI thread could each drop the other's change.
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut settings = Settings::load();
        // Write only the files the edit moved.
        let before = settings.prints();
        f(&mut settings);
        settings.save_changed(&before);
    }

    fn prints(&self) -> Shards {
        Shards {
            core: serde_json::to_string(self).ok(),
            look: serde_json::to_string(&self.look).ok(),
            windows: serde_json::to_string(&self.windows).ok(),
            session: serde_json::to_string(&self.session).ok(),
            accounts: serde_json::to_string(&self.accounts).ok(),
        }
    }

    /// Not atomic across files: a crash partway leaves one an edit behind,
    /// never corrupt.
    fn save_changed(&self, before: &Shards) {
        let after = self.prints();
        let forced = self.migrated;
        write_shard(
            settings_path(),
            "settings",
            &before.core,
            &after.core,
            forced,
            self,
        );
        write_shard(
            look_path(),
            "look",
            &before.look,
            &after.look,
            forced,
            &self.look,
        );
        write_shard(
            windows_path(),
            "windows",
            &before.windows,
            &after.windows,
            forced,
            &self.windows,
        );
        write_shard(
            session_path(),
            "session",
            &before.session,
            &after.session,
            forced,
            &self.session,
        );
        write_shard(
            accounts_path(),
            "accounts",
            &before.accounts,
            &after.accounts,
            forced,
            &self.accounts,
        );
    }

    pub fn palette_dark(&self) -> Palette {
        Palette::from_map(&self.look.bundle.palette_dark)
    }

    pub fn palette_light(&self) -> Palette {
        Palette::from_map_over(Palette::light(), &self.look.bundle.palette_light)
    }

    pub fn palette_map_mut(&mut self, mode: palette::Mode) -> &mut BTreeMap<String, String> {
        match mode {
            palette::Mode::Dark => &mut self.look.bundle.palette_dark,
            palette::Mode::Light => &mut self.look.bundle.palette_light,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rox-portable-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn portable_takes_a_writable_exe_folder() {
        let dir = scratch("writable");
        let (chosen, portable) = choose_data_dir(true, Some(&dir));
        assert!(portable);
        assert_eq!(chosen, dir.join(PORTABLE_DATA));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stock_run_ignores_the_exe_folder() {
        let dir = scratch("stock");
        let (chosen, portable) = choose_data_dir(false, Some(&dir));
        assert!(!portable);
        assert!(!chosen.starts_with(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn portable_falls_back_when_the_exe_folder_is_read_only() {
        let dir = scratch("readonly");
        let mut perms = std::fs::metadata(&dir).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&dir, perms.clone()).unwrap();

        // Root writes anywhere, so there's nothing to check under it.
        if dir_writable(&dir) {
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(&dir, perms);
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        let (chosen, portable) = choose_data_dir(true, Some(&dir));
        assert!(!portable);
        assert!(!chosen.starts_with(&dir));

        perms.set_readonly(false);
        let _ = std::fs::set_permissions(&dir, perms);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn portable_without_an_exe_folder_falls_back() {
        let (_, portable) = choose_data_dir(true, None);
        assert!(!portable);
    }

    #[test]
    fn milkdrop_picker_state_reads_size_only_entry() {
        let state: MilkdropPickerWindowState =
            serde_json::from_str(r#"{"width": 560.0, "height": 640.0}"#).unwrap();
        assert_eq!(state.width, 560.0);
        assert_eq!(state.height, 640.0);
        assert!(!state.favorites_only);
        assert!(state.nested);
    }

    fn dressed() -> Settings {
        let mut src = Settings {
            theme: Theme::Light,
            app_font_size: 20.0,
            ..Default::default()
        };
        let look = &mut src.look.bundle;
        look.primary_layout = Some("one".into());
        look.palette_dark.insert("accent".into(), "#336699".into());
        look.palette_light.insert("accent".into(), "#663399".into());
        look.layouts.push(NamedLayout {
            name: "one".into(),
            dump: serde_json::json!({ "k": "v" }),
            size: None,
        });
        look.appearance.surface_opacity = 0.5;
        look.appearance.frame = Frame {
            margin: Sides::all(4.0),
            padding: Sides::all(8.0),
            rounding: 12.0,
            border: Sides::all(1.0),
        };
        look.appearance.art_theming = true;
        look.appearance.keep_theme = true;
        look.appearance.rating_style = RatingStyle::Numeric;
        look.appearance.rating_dots = true;
        look.appearance.hide_menubar = true;
        src
    }

    #[test]
    fn workspace_bundle_roundtrips() {
        let src = dressed();
        let bundle = WorkspaceBundle::from_settings("mine".into(), &src);
        let json = serde_json::to_string(&bundle).unwrap();
        let back: WorkspaceBundle = serde_json::from_str(&json).unwrap();

        let mut dst = Settings::default();
        back.apply_to(&mut dst);
        let look = &dst.look.bundle;
        assert_eq!(look.name, "mine");
        assert_eq!(look.appearance.surface_opacity, 0.5);
        assert_eq!(look.appearance.frame.rounding, 12.0);
        assert_eq!(look.appearance.frame.padding, Sides::all(8.0));
        assert!(dst.theme == Theme::default());
        assert_eq!(dst.app_font_size, Settings::default().app_font_size);
        assert!(look.appearance.art_theming);
        assert!(look.appearance.keep_theme);
        assert!(look.appearance.rating_style == RatingStyle::Numeric);
        assert!(look.appearance.rating_dots);
        assert!(look.appearance.hide_menubar);
        assert_eq!(
            look.palette_dark.get("accent").map(String::as_str),
            Some("#336699")
        );
        assert_eq!(
            look.palette_light.get("accent").map(String::as_str),
            Some("#663399")
        );
        assert_eq!(look.layouts.len(), 1);
        assert_eq!(look.primary_layout.as_deref(), Some("one"));
    }

    #[test]
    fn workspace_bundle_omits_machine_state() {
        let bundle = WorkspaceBundle::from_settings("mine".into(), &Settings::default());
        let json = serde_json::to_string(&bundle).unwrap();
        assert!(!json.contains("library_root"));
        assert!(!json.contains("lastfm"));
        assert!(!json.contains("session_key"));
        assert!(!json.contains("last_track"));
    }

    /// Last.fm binds a session to the key that minted it, so each install
    /// has to find its own.
    #[test]
    fn a_session_belongs_to_the_key_that_minted_it() {
        let mut lastfm = Lastfm::default();
        lastfm.connect("nix-key", "sk-nix".into(), "zealsprince".into());
        assert_eq!(
            lastfm.session("nix-key").map(|s| s.key.as_str()),
            Some("sk-nix")
        );
        assert!(lastfm.session("release-key").is_none());
        assert!(lastfm.connected_elsewhere("release-key"));

        lastfm.connect("release-key", "sk-release".into(), "zealsprince".into());
        assert_eq!(
            lastfm.session("nix-key").map(|s| s.key.as_str()),
            Some("sk-nix")
        );
        assert_eq!(
            lastfm.session("release-key").map(|s| s.key.as_str()),
            Some("sk-release")
        );
    }

    /// One global bound would make a second account's import fetch nothing
    /// and file its history as dateless estimates.
    #[test]
    fn each_account_carries_its_own_import_bound() {
        let mut lastfm = Lastfm::default();
        assert_eq!(lastfm.imported_through("catlinman"), None);

        lastfm.note_import("zealsprince", 1_700_000_000);
        assert_eq!(
            lastfm.imported_through("catlinman"),
            None,
            "one account's history says nothing about another's"
        );
        assert_eq!(lastfm.imported_through("zealsprince"), Some(1_700_000_000));
        assert_eq!(
            lastfm.imported_through("ZealSprince"),
            Some(1_700_000_000),
            "and Last.fm's own casing of a name is the same account"
        );

        // A stopped run can't pull the bound back down.
        lastfm.note_import("zealsprince", 1_600_000_000);
        assert_eq!(lastfm.imported_through("zealsprince"), Some(1_700_000_000));

        lastfm.forget_imports();
        assert_eq!(lastfm.imported_through("zealsprince"), None);
    }

    #[test]
    fn the_unattributed_session_goes_to_whoever_proves_it_works() {
        let mut lastfm: Lastfm = serde_json::from_value(serde_json::json!({
            "session_key": "sk-old",
            "username": "zealsprince",
        }))
        .unwrap();
        lastfm.fold_legacy_session();
        assert_eq!(
            lastfm.session("nix-key").map(|s| s.key.as_str()),
            Some("sk-old")
        );
        assert_eq!(
            lastfm.session("release-key").map(|s| s.key.as_str()),
            Some("sk-old")
        );

        assert!(lastfm.attribute("nix-key"));
        assert_eq!(
            lastfm.session("nix-key").map(|s| s.key.as_str()),
            Some("sk-old")
        );
        assert!(lastfm.session("release-key").is_none());
        assert!(
            !lastfm.attribute("release-key"),
            "there is nothing left to claim"
        );
    }

    /// Without the empty entry, every launch retries a session that isn't
    /// this build's.
    #[test]
    fn a_refused_key_stops_reaching_for_a_session_that_isnt_its_own() {
        let mut lastfm: Lastfm = serde_json::from_value(serde_json::json!({
            "session_key": "sk-old",
            "username": "zealsprince",
        }))
        .unwrap();
        lastfm.fold_legacy_session();
        lastfm.clear_session("release-key");
        assert!(lastfm.session("release-key").is_none());
        assert_eq!(
            lastfm.session("nix-key").map(|s| s.key.as_str()),
            Some("sk-old")
        );

        let back: Lastfm = serde_json::from_str(&serde_json::to_string(&lastfm).unwrap()).unwrap();
        assert!(back.session("release-key").is_none());
    }

    #[test]
    fn disconnecting_doesnt_fall_back_to_someone_elses_session() {
        let mut lastfm: Lastfm = serde_json::from_value(serde_json::json!({
            "session_key": "sk-old",
            "username": "zealsprince",
        }))
        .unwrap();
        lastfm.fold_legacy_session();
        lastfm.connect("release-key", "sk-release".into(), "zealsprince".into());
        lastfm.clear_session("release-key");
        assert!(lastfm.session("release-key").is_none());
    }

    #[test]
    fn a_single_subsonic_server_becomes_the_first_of_the_list() {
        let mut accounts: AccountsState = serde_json::from_value(serde_json::json!({
            "subsonic": {
                "enabled": true,
                "url": "https://music.example.com",
                "user": "andrew",
                "password": "pw",
                "last_sync": 1_700_000_000,
            },
        }))
        .unwrap();
        accounts.fold_legacy_subsonic();

        assert_eq!(accounts.subsonic_servers.len(), 1);
        let server = &accounts.subsonic_servers[0];
        assert!(server.enabled);
        assert_eq!(server.url, "https://music.example.com");
        assert_eq!(server.password, "pw");
        assert_eq!(server.last_sync, 1_700_000_000);

        let json = serde_json::to_value(&accounts).unwrap();
        assert!(json.get("subsonic").is_none());
        assert!(json.get("subsonic_servers").is_some());
    }

    #[test]
    fn the_legacy_subsonic_server_never_overrides_a_list() {
        let mut listed: AccountsState = serde_json::from_value(serde_json::json!({
            "subsonic": { "url": "https://old.example.com" },
            "subsonic_servers": [{ "url": "https://new.example.com" }],
        }))
        .unwrap();
        listed.fold_legacy_subsonic();

        assert_eq!(listed.subsonic_servers.len(), 1);
        assert_eq!(listed.subsonic_servers[0].url, "https://new.example.com");

        let mut blank: AccountsState = serde_json::from_value(serde_json::json!({
            "subsonic": { "enabled": true, "url": "  " },
        }))
        .unwrap();
        blank.fold_legacy_subsonic();

        assert!(blank.subsonic_servers.is_empty());
    }

    #[test]
    fn a_subsonic_server_is_labelled_by_name_then_host() {
        let mut account = SubsonicAccount {
            url: "https://music.example.com:4533/navidrome/".into(),
            ..SubsonicAccount::default()
        };
        assert_eq!(account.label(), "music.example.com:4533");

        account.name = "  Home  ".into();
        assert_eq!(account.label(), "Home");

        let bare = SubsonicAccount {
            url: "music.example.com/rest".into(),
            ..SubsonicAccount::default()
        };
        assert_eq!(bare.label(), "music.example.com");
        assert_eq!(SubsonicAccount::default().label(), "");
    }

    #[test]
    fn no_api_key_means_no_session() {
        let mut lastfm = Lastfm::default();
        lastfm.connect("nix-key", "sk-nix".into(), "zealsprince".into());
        assert!(lastfm.session("").is_none());
        assert_eq!(lastfm.username(""), "");

        // A refusal here would clobber the unattributed slot.
        let mut carried: Lastfm = serde_json::from_value(serde_json::json!({
            "session_key": "sk-old",
        }))
        .unwrap();
        carried.fold_legacy_session();
        carried.clear_session("");
        assert_eq!(
            carried.session("nix-key").map(|s| s.key.as_str()),
            Some("sk-old")
        );
    }

    #[test]
    fn workspace_bundle_carries_its_shader_pool() {
        let mut bundle = WorkspaceBundle {
            shaders: vec![
                NamedShader {
                    name: "Grain".to_string(),
                    source: "fn fs_user(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }"
                        .to_string(),
                    path: Some(PathBuf::from("/home/someone/grain.wgsl")),
                    assets: Vec::new(),
                },
                NamedShader {
                    name: "Bloom".to_string(),
                    source: "// bloom".to_string(),
                    path: None,
                    assets: Vec::new(),
                },
            ],
            ..WorkspaceBundle::default()
        };

        let live = serde_json::to_value(&bundle).unwrap();
        assert_eq!(live["shaders"][0]["path"], "/home/someone/grain.wgsl");
        assert!(
            live["shaders"][1].get("path").is_none(),
            "an unejected entry writes no bookmark: {live}"
        );

        bundle.scrub_paths();
        let json = serde_json::to_string(&bundle).unwrap();
        let back: WorkspaceBundle = serde_json::from_str(&json).unwrap();
        assert_eq!(back.shaders.len(), 2);
        assert_eq!(back.shaders[0].name, "Grain");
        assert!(back.shaders[0].source.contains("fs_user"));
        assert!(
            back.shaders[0].path.is_none(),
            "the bookmark shouldn't have travelled"
        );
        assert_eq!(back.shaders[1].name, "Bloom");
    }

    #[test]
    fn a_broken_pool_entry_costs_only_itself() {
        let json = serde_json::json!({
            "shaders": [
                { "name": "Grain", "source": "// grain" },
                { "name": "Bloom", "source": 7 },
            ],
        });
        let bundle: WorkspaceBundle = serde_json::from_value(json).unwrap();
        assert_eq!(bundle.shaders.len(), 1);
        assert_eq!(bundle.shaders[0].name, "Grain");
    }

    #[test]
    fn shader_assets_ride_the_pool_entry() {
        let plain = serde_json::to_value(NamedShader {
            name: "Grain".to_string(),
            source: "// grain".to_string(),
            path: None,
            assets: Vec::new(),
        })
        .unwrap();
        assert!(
            plain.get("assets").is_none(),
            "a shader with no plates writes no key: {plain}"
        );

        let plate = [0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0x00];
        let mut bundle = WorkspaceBundle {
            shaders: vec![NamedShader {
                name: "Serpent".to_string(),
                source: "// @asset plate: plate.png".to_string(),
                path: Some(PathBuf::from("/home/someone/serpent.wgsl")),
                assets: vec![ShaderAsset::from_bytes("plate.png", &plate)],
            }],
            ..WorkspaceBundle::default()
        };

        bundle.scrub_paths();
        let json = serde_json::to_string(&bundle).unwrap();
        let back: WorkspaceBundle = serde_json::from_str(&json).unwrap();
        let entry = &back.shaders[0];
        assert!(
            entry.path.is_none(),
            "the bookmark shouldn't have travelled"
        );
        assert_eq!(entry.assets.len(), 1);
        assert_eq!(entry.assets[0].file, "plate.png");
        assert_eq!(
            entry.assets[0].decode().unwrap(),
            plate,
            "the plate landed as the same file that went in"
        );
    }

    #[test]
    fn a_broken_asset_costs_only_itself() {
        let json = serde_json::json!({
            "shaders": [{
                "name": "Serpent",
                "source": "// serpent",
                "assets": [
                    { "file": "plate.png", "data": "AAEC" },
                    { "file": "dither.png", "data": 7 },
                ],
            }],
        });
        let bundle: WorkspaceBundle = serde_json::from_value(json).unwrap();
        let assets = &bundle.shaders[0].assets;
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].decode().unwrap(), vec![0u8, 1, 2]);

        let bad = ShaderAsset {
            file: "plate.png".to_string(),
            data: "not base64!".to_string(),
        };
        assert!(bad.decode().is_err());
    }

    #[test]
    fn workspace_meta_and_post_shader_ride_the_bundle() {
        let plain = serde_json::to_value(WorkspaceBundle::default()).unwrap();
        assert!(plain.get("meta").is_none(), "an empty card writes no key");
        assert!(
            plain.get("post_shader").is_none(),
            "no screen shader writes no key"
        );
        assert!(
            plain.get("shaders").is_none(),
            "an empty pool writes no key"
        );

        let bundle = WorkspaceBundle {
            meta: WorkspaceMeta {
                author: "Andrew".to_string(),
                description: "Warm and quiet.".to_string(),
                website: "https://zealsprince.com".to_string(),
                version: "2".to_string(),
                license: "CC BY 4.0".to_string(),
                created: "2026-01-02".to_string(),
                updated: "2026-08-07".to_string(),
            },
            post_shader: Some(PostShaderConfig {
                enabled: true,
                source: "// crt".to_string(),
                name: Some("Grain".to_string()),
                all_windows: true,
                ..PostShaderConfig::default()
            }),
            ..WorkspaceBundle::default()
        };
        let json = serde_json::to_string(&bundle).unwrap();
        let back: WorkspaceBundle = serde_json::from_str(&json).unwrap();
        assert_eq!(back.meta.author, "Andrew");
        assert_eq!(back.meta.website, "https://zealsprince.com");
        assert_eq!(back.meta.license, "CC BY 4.0");
        assert_eq!(back.meta.created, "2026-01-02");
        assert_eq!(back.meta.updated, "2026-08-07");
        let post = back.post_shader.expect("the screen shader travels");
        assert!(post.enabled);
        assert!(post.all_windows);
        assert_eq!(post.source, "// crt");
        assert_eq!(post.name.as_deref(), Some("Grain"));

        let older: WorkspaceBundle =
            serde_json::from_value(serde_json::json!({ "version": 1, "name": "old" })).unwrap();
        assert!(older.shaders.is_empty());
        assert!(older.meta.is_empty());
        assert!(older.post_shader.is_none());
    }

    #[test]
    fn a_card_is_created_once_and_updated_always() {
        let mut meta = WorkspaceMeta::default();
        assert!(meta.is_empty());

        meta.stamp("2026-01-02");
        assert_eq!(meta.created, "2026-01-02");
        assert_eq!(meta.updated, "2026-01-02");
        assert!(!meta.is_empty(), "a stamped card is a card");

        meta.stamp("2026-08-07");
        assert_eq!(meta.created, "2026-01-02");
        assert_eq!(meta.updated, "2026-08-07");

        let described = WorkspaceMeta {
            description: "Warm and quiet.".to_string(),
            ..WorkspaceMeta::default()
        };
        assert!(!described.is_empty());
    }

    /// A live look with its own card wins field by field, so a fork isn't
    /// signed by the person you forked from.
    #[test]
    fn carry_forward_keeps_a_card_through_an_overwrite() {
        let prior = WorkspaceMeta {
            author: "Nova".into(),
            description: "Warm and quiet.".into(),
            website: "example.com".into(),
            version: "1.0".into(),
            license: "CC BY".into(),
            created: "2026-01-02".into(),
            updated: "2026-03-04".into(),
        };

        let mut fresh = WorkspaceMeta::default();
        fresh.stamp("2026-08-07");
        fresh.carry_forward(&prior);
        assert_eq!(fresh.author, "Nova");
        assert_eq!(fresh.description, "Warm and quiet.");
        assert_eq!(fresh.website, "example.com");
        assert_eq!(fresh.version, "1.0");
        assert_eq!(fresh.license, "CC BY");
        assert_eq!(fresh.created, "2026-01-02", "the first day survives");
        assert_eq!(fresh.updated, "2026-08-07", "today stays on updated");

        let mut mine = WorkspaceMeta {
            author: "Juniper".into(),
            version: "2".into(),
            ..WorkspaceMeta::default()
        };
        mine.stamp("2026-08-07");
        mine.carry_forward(&prior);
        assert_eq!(mine.author, "Juniper");
        assert_eq!(mine.version, "2");
        assert_eq!(mine.license, "CC BY");

        let mut alone = WorkspaceMeta::default();
        alone.stamp("2026-08-07");
        alone.carry_forward(&WorkspaceMeta::default());
        assert_eq!(alone.created, "2026-08-07");
        assert!(alone.author.is_empty());
    }

    #[test]
    fn from_settings_stamps_the_card() {
        let bundle = WorkspaceBundle::from_settings("mine".into(), &Settings::default());
        let today = utc_today();
        assert_eq!(bundle.meta.created, today);
        assert_eq!(bundle.meta.updated, today);
        // A reader elsewhere parses this as a date.
        assert_eq!(today.len(), 10);
        assert!(today.chars().all(|c| c.is_ascii_digit() || c == '-'));
    }

    #[test]
    fn from_settings_takes_the_screen_shader_along() {
        let file = std::env::temp_dir().join("rox-test-from-settings-shader.wgsl");
        std::fs::write(&file, "// scanlines\n").expect("write the working copy");

        let src = Settings {
            post_shader: PostShaderConfig {
                enabled: true,
                path: Some(file.clone()),
                all_windows: true,
                ..PostShaderConfig::default()
            },
            ..Settings::default()
        };
        let post = WorkspaceBundle::from_settings("mine".into(), &src)
            .post_shader
            .expect("the screen shader travels");
        assert_eq!(post.source, "// scanlines\n");
        assert!(post.path.is_none(), "the bookmark doesn't travel");
        assert!(post.enabled);
        assert!(post.all_windows);

        let parked = Settings {
            post_shader: PostShaderConfig {
                path: Some(file.clone()),
                ..PostShaderConfig::default()
            },
            ..Settings::default()
        };
        assert!(
            WorkspaceBundle::from_settings("mine".into(), &parked)
                .post_shader
                .is_some()
        );

        assert!(
            WorkspaceBundle::from_settings("mine".into(), &Settings::default())
                .post_shader
                .is_none(),
            "an untouched default is nothing to carry"
        );

        std::fs::remove_file(&file).ok();
    }

    /// Both names go through the filename sanitizer, so an entry called
    /// "Grain / Fine" can't write into a folder nobody asked for.
    #[test]
    fn shader_ejects_under_its_workspace() {
        let root = Path::new("/tmp/rox-shaders");
        assert_eq!(
            shader_eject_path_in(root, "Nightfall", "Grain"),
            root.join("Nightfall").join("Grain.wgsl")
        );
        assert_eq!(
            shader_eject_path_in(root, "Live/Studio", "Grain / Fine"),
            root.join("Live Studio").join("Grain   Fine.wgsl")
        );
        assert_eq!(
            shader_eject_path_in(root, "", "Grain"),
            root.join("_local").join("Grain.wgsl")
        );
        assert_eq!(
            shader_eject_path_in(root, "...", "..."),
            root.join("_local").join("shader.wgsl")
        );
        assert_eq!(safe_file_stem("  padded  ", "fallback"), "padded");
        assert_eq!(safe_file_stem(".hidden", "fallback"), "hidden");
    }

    /// The trust pass must find sources where the scrub finds bookmarks, or a
    /// shipped look's panels wait on an approval nobody can give.
    #[test]
    fn dump_shader_sources_finds_both_shapes() {
        let dump = serde_json::json!({
            "panel_name": "StackPanel",
            "children": [
                {
                    "panel_name": "shader",
                    "info": { "panel": {
                        "source": "// the shader panel",
                        "path": "/home/someone/panel.wgsl",
                    }},
                },
                {
                    "panel_name": "folder tree",
                    "info": { "panel": {
                        "path": "/home/someone/Music",
                        "shader": { "enabled": true, "source": "// the surface one" },
                    }},
                },
            ],
        });
        let mut found = dump_shader_sources(&dump);
        found.sort();
        assert_eq!(found, ["// the shader panel", "// the surface one"]);
    }

    #[test]
    fn a_dump_knows_when_it_wears_a_shader() {
        let worn = |shader: serde_json::Value| {
            serde_json::json!({
                "panel_name": "folder tree",
                "info": { "panel": { "shader": shader }},
            })
        };
        assert!(dump_wears_shader(&worn(
            serde_json::json!({ "enabled": true, "source": "// inline" })
        )));
        assert!(dump_wears_shader(&worn(
            serde_json::json!({ "enabled": true, "source": "", "name": "Lace" })
        )));
        assert!(!dump_wears_shader(&worn(
            serde_json::json!({ "enabled": false, "source": "// switched off" })
        )));
        assert!(!dump_wears_shader(&worn(
            serde_json::json!({ "enabled": true, "source": "  " })
        )));
        assert!(!dump_wears_shader(&serde_json::json!({
            "panel_name": "folder tree",
            "info": { "panel": { "path": "/home/someone/Music" }},
        })));
        assert!(dump_wears_shader(&serde_json::json!({
            "panel_name": "shader",
            "info": { "panel": {}},
        })));
    }

    #[test]
    fn stripping_a_dump_parks_both_shapes() {
        let mut dump = serde_json::json!({
            "panel_name": "StackPanel",
            "children": [
                {
                    "panel_name": "shader",
                    "info": { "panel": {
                        "source": "// the shader panel",
                        "name": "Lace",
                        "run_when_idle": true,
                    }},
                },
                {
                    "panel_name": "folder tree",
                    "info": { "panel": {
                        "path": "/home/someone/Music",
                        "shader": { "enabled": true, "name": "Lace" },
                    }},
                },
            ],
        });
        strip_dump_shaders(&mut dump);
        assert!(!dump_wears_shader(&dump));
        // Sources stay, so the trust walk still sees them.
        assert_eq!(dump_shader_sources(&dump), vec!["// the shader panel"]);
        let panels = &dump["children"];
        assert_eq!(panels[0]["info"]["panel"]["enabled"], false);
        assert_eq!(panels[0]["info"]["panel"]["name"], "Lace");
        assert_eq!(panels[0]["info"]["panel"]["source"], "// the shader panel");
        assert_eq!(panels[0]["info"]["panel"]["run_when_idle"], true);
        assert_eq!(panels[1]["info"]["panel"]["shader"]["enabled"], false);
        assert_eq!(panels[1]["info"]["panel"]["shader"]["name"], "Lace");
        assert_eq!(panels[1]["info"]["panel"]["path"], "/home/someone/Music");
    }

    #[test]
    fn stripping_parks_a_shader_panel_with_no_config() {
        let mut dump = serde_json::json!({ "panel_name": "shader" });
        strip_dump_shaders(&mut dump);
        assert!(!dump_wears_shader(&dump));
        assert_eq!(dump["info"]["panel"]["enabled"], false);
    }

    #[test]
    fn an_export_inlines_the_screen_shader() {
        let file = std::env::temp_dir().join("rox-test-export-shader.wgsl");
        std::fs::write(&file, "// crt\n").expect("write the working copy");

        let mut bundle = WorkspaceBundle {
            post_shader: Some(PostShaderConfig {
                enabled: true,
                path: Some(file.clone()),
                ..PostShaderConfig::default()
            }),
            ..WorkspaceBundle::default()
        };
        bundle.inline_post_shader();
        bundle.scrub_paths();
        let post = bundle.post_shader.clone().expect("still there");
        assert_eq!(post.source, "// crt\n");
        assert!(post.path.is_none(), "the bookmark doesn't travel");

        let mut kept = WorkspaceBundle {
            post_shader: Some(PostShaderConfig {
                source: "// what runs".to_string(),
                path: Some(file.clone()),
                ..PostShaderConfig::default()
            }),
            ..WorkspaceBundle::default()
        };
        kept.inline_post_shader();
        assert_eq!(kept.post_shader.unwrap().source, "// what runs");

        std::fs::remove_file(&file).ok();

        let mut missing = WorkspaceBundle {
            post_shader: Some(PostShaderConfig {
                path: Some(file),
                ..PostShaderConfig::default()
            }),
            ..WorkspaceBundle::default()
        };
        missing.inline_post_shader();
        assert!(missing.post_shader.unwrap().source.is_empty());
    }

    /// A folder panel's root is a `path` too, and must survive the scrub.
    #[test]
    fn scrub_paths_takes_only_the_shader_bookmarks() {
        let mut bundle = WorkspaceBundle {
            layouts: vec![NamedLayout {
                name: "one".to_string(),
                size: None,
                dump: serde_json::json!({
                    "panel_name": "StackPanel",
                    "children": [
                        {
                            "panel_name": "shader",
                            "children": [],
                            "info": { "panel": {
                                "source": "// the panel's own",
                                "path": "/home/someone/panel.wgsl",
                                "routes": [],
                            }},
                        },
                        {
                            "panel_name": "folder tree",
                            "children": [],
                            "info": { "panel": {
                                "path": "/home/someone/Music",
                                "shader": {
                                    "enabled": true,
                                    "source": "// the surface one",
                                    "path": "/home/someone/surface.wgsl",
                                },
                            }},
                        },
                    ],
                    "info": { "stack": { "sizes": [], "axis": 0 } },
                }),
            }],
            ..WorkspaceBundle::default()
        };
        bundle.scrub_paths();

        let dump = &bundle.layouts[0].dump;
        let shader_panel = &dump["children"][0]["info"]["panel"];
        assert!(shader_panel.get("path").is_none(), "{dump}");
        assert_eq!(shader_panel["source"], "// the panel's own");

        let folder = &dump["children"][1]["info"]["panel"];
        assert_eq!(
            folder["path"], "/home/someone/Music",
            "a folder panel's root is not a shader bookmark: {dump}"
        );
        assert!(folder["shader"].get("path").is_none(), "{dump}");
        assert_eq!(folder["shader"]["source"], "// the surface one");
    }

    #[test]
    fn the_shader_pool_answers_by_name_and_bumps_its_rev() {
        let before = shader_pool_rev();
        note_shader_pool(vec![NamedShader {
            name: "Grain".to_string(),
            source: "// grain".to_string(),
            path: None,
            assets: Vec::new(),
        }]);
        assert!(shader_pool_rev() > before, "a replacement is news");
        assert_eq!(shader_pool().len(), 1);
        assert_eq!(
            shader_pool_get("Grain").map(|s| s.source),
            Some("// grain".to_string())
        );
        assert!(shader_pool_get("Bloom").is_none());

        let between = shader_pool_rev();
        note_shader_pool(Vec::new());
        assert!(shader_pool_rev() > between);
        assert!(shader_pool().is_empty());
        assert!(shader_pool_get("Grain").is_none(), "an apply replaces it");
    }

    #[test]
    fn shader_approved_trusts_what_the_build_ships() {
        let print = "shipped-with-the-binary-not-a-real-hash";
        assert!(!shader_approved(print));
        trust_shipped([print.to_string()]);
        assert!(shader_approved(print));
        assert!(!APPROVED_SHADERS.read().unwrap().contains(print));
    }

    #[test]
    fn frame_clamps_each_knob_to_its_ceiling() {
        let clamped = Frame {
            margin: Sides::all(MARGIN_MAX + 100.0),
            padding: Sides::all(-5.0),
            rounding: ROUNDING_MAX + 1.0,
            border: Sides::all(1.0).with(palette::Side::Top, BORDER_MAX + 10.0),
        }
        .clamped();
        assert_eq!(clamped.margin, Sides::all(MARGIN_MAX));
        assert_eq!(clamped.padding, Sides::ZERO);
        assert_eq!(clamped.rounding, ROUNDING_MAX);
        assert_eq!(
            clamped.border,
            Sides::all(1.0).with(palette::Side::Top, BORDER_MAX)
        );
    }

    #[test]
    fn frame_resets_non_finite_knobs() {
        let clamped = Frame {
            margin: Sides::all(f32::NAN),
            padding: Sides::all(f32::INFINITY),
            rounding: 6.0,
            border: Sides::all(2.0),
        }
        .clamped();
        assert_eq!(clamped.margin, Sides::ZERO);
        assert_eq!(clamped.padding, Sides::ZERO);
        assert_eq!(clamped.rounding, 6.0);
        assert_eq!(clamped.border, Sides::all(2.0));
    }

    #[test]
    fn settings_roundtrip_preserves_fields() {
        let mut src = Settings {
            theme: Theme::Light,
            app_font_size: 20.0,
            watch_library: false,
            fold_case: true,
            quit_to_tray: true,
            ..Default::default()
        };
        src.library_roots.push(PathBuf::from("/music"));

        let json = serde_json::to_string_pretty(&src).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert!(back.theme == Theme::Light);
        assert_eq!(back.app_font_size, 20.0);
        assert!(!back.watch_library);
        assert!(back.fold_case);
        assert!(back.quit_to_tray);
        assert_eq!(back.library_roots, vec![PathBuf::from("/music")]);
    }

    #[test]
    fn post_shader_round_trips_and_defaults_off() {
        let mut src = Settings::default();
        src.post_shader.enabled = true;
        src.post_shader.path = Some(PathBuf::from("/shaders/crt.wgsl"));
        src.post_shader.all_windows = true;
        let json = serde_json::to_string_pretty(&src).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert!(back.post_shader.enabled);
        assert_eq!(
            back.post_shader.path,
            Some(PathBuf::from("/shaders/crt.wgsl"))
        );
        assert!(back.post_shader.all_windows);

        let older: Settings = serde_json::from_str(r#"{"theme":"light"}"#).unwrap();
        assert!(!older.post_shader.enabled);
        assert!(older.post_shader.path.is_none());
        assert!(!older.post_shader.all_windows);
        assert!(older.post_shader.routes.is_empty());
    }

    #[test]
    fn post_shader_routes_round_trip_and_stay_out_of_older_files() {
        let mut src = Settings::default();
        src.post_shader.routes = vec![Route {
            enabled: true,
            signal: 7,
            target: "slot3".to_string(),
            from: 0.25,
            to: 1.5,
        }];
        let json = serde_json::to_string_pretty(&src).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.post_shader.routes.len(), 1);
        assert_eq!(back.post_shader.routes[0].signal, 7);
        assert_eq!(back.post_shader.routes[0].target, "slot3");
        assert_eq!(back.post_shader.routes[0].to, 1.5);

        let bare = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!bare.contains("routes"));

        let older: Settings = serde_json::from_str(r#"{"post_shader":{"enabled":true}}"#).unwrap();
        assert!(older.post_shader.routes.is_empty());
    }

    #[test]
    fn replay_gain_save_round_trips_and_defaults_to_the_database() {
        let mut src = Settings::default();
        src.replay_gain.save = ReplayGainSave::Tags;
        let json = serde_json::to_string_pretty(&src).unwrap();
        assert!(json.contains("\"save\": \"tags\""), "{json}");
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.replay_gain.save, ReplayGainSave::Tags);

        let older: ReplayGainSettings = serde_json::from_str(r#"{"mode":"track"}"#).unwrap();
        assert_eq!(older.save, ReplayGainSave::Database);
    }

    /// A missing or unknown value falls back to the database. Neither may
    /// read as permission to rewrite everyone's tags.
    #[test]
    fn acoustic_save_round_trips_and_defaults_to_the_database() {
        let mut src = Settings::default();
        assert_eq!(src.acoustic_save, AcousticSave::Database);
        src.acoustic_save = AcousticSave::Tags;
        let json = serde_json::to_string_pretty(&src).unwrap();
        assert!(json.contains("\"acoustic_save\": \"tags\""), "{json}");
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.acoustic_save, AcousticSave::Tags);

        let older: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(older.acoustic_save, AcousticSave::Database);
        let newer: Settings = serde_json::from_str(r#"{"acoustic_save":"cloud"}"#).unwrap();
        assert_eq!(newer.acoustic_save, AcousticSave::Database);
    }

    #[test]
    fn replay_gain_auto_round_trips_and_defaults_to_off() {
        let mut src = Settings::default();
        assert!(!src.replay_gain.auto);
        src.replay_gain.auto = true;
        let json = serde_json::to_string_pretty(&src).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert!(back.replay_gain.auto);

        let older: ReplayGainSettings = serde_json::from_str(r#"{"mode":"track"}"#).unwrap();
        assert!(!older.auto);
    }

    #[test]
    fn shard_files_roundtrip() {
        let mut session = SessionState {
            volume: 1.5,
            muted: true,
            shuffle: true,
            last_scan: 12345,
            ..Default::default()
        };
        session.set_loop_mode(LoopMode::All);
        let back: SessionState =
            serde_json::from_str(&serde_json::to_string(&session).unwrap()).unwrap();
        assert_eq!(back.volume, 1.5);
        assert!(back.muted);
        assert!(back.shuffle);
        assert!(back.loop_mode() == LoopMode::All);
        assert_eq!(back.last_scan, 12345);

        let mut accounts = AccountsState::default();
        accounts
            .lastfm
            .connect("api-key", "sk".into(), "zealsprince".into());
        accounts.listenbrainz.token = "a-user-token".into();
        accounts.listenbrainz.username = Some("zealsprince".into());
        let back: AccountsState =
            serde_json::from_str(&serde_json::to_string(&accounts).unwrap()).unwrap();
        assert_eq!(back.lastfm.username("api-key"), "zealsprince");
        assert_eq!(back.listenbrainz.token, "a-user-token");
        assert_eq!(back.listenbrainz.username.as_deref(), Some("zealsprince"));

        let windows = WindowsState {
            main: Some(WindowState {
                x: 10.0,
                y: 20.0,
                width: 800.0,
                height: 600.0,
                maximized: true,
            }),
            queue_view: Some(serde_json::json!({ "columns": ["title"] })),
            ..Default::default()
        };
        let back: WindowsState =
            serde_json::from_str(&serde_json::to_string(&windows).unwrap()).unwrap();
        assert_eq!(back.main.map(|w| w.width), Some(800.0));
        assert!(back.queue_view.is_some());
    }

    /// The settings file is the one people are pointed at, so it must carry no
    /// shard, above all no credentials.
    #[test]
    fn settings_file_carries_only_preferences() {
        let mut src = dressed();
        src.accounts
            .lastfm
            .connect("api-key", "a-real-secret".into(), "zealsprince".into());
        src.session.volume = 0.5;
        src.windows.main = Some(WindowState {
            x: 1.0,
            y: 2.0,
            width: 800.0,
            height: 600.0,
            maximized: false,
        });
        let json = serde_json::to_string(&src).unwrap();
        for key in [
            // the look
            "look",
            "layouts",
            "palette_dark",
            "surface_opacity",
            "rating_style",
            "workspaces",
            // Quote-anchored: `all_windows` legitimately contains the substring.
            "\"windows\"",
            "main",
            "tag_editor",
            "queue_view",
            // the session
            "session",
            "volume",
            "muted",
            "shuffle",
            "last_queue",
            "last_scan",
            // the accounts
            "accounts",
            "lastfm",
            "providers",
            "discord",
            "sessions",
            "a-real-secret",
        ] {
            assert!(!json.contains(key), "settings.json still carries {key}");
        }
    }

    #[test]
    fn the_legacy_threshold_seeds_the_shared_knob_once() {
        let mut settings = Settings::default();
        settings.accounts.lastfm = serde_json::from_value(serde_json::json!({
            "threshold": 0.8
        }))
        .unwrap();
        seed_threshold(&mut settings, &serde_json::json!({}));
        assert_eq!(settings.scrobble_threshold, 0.8);
        assert!(
            settings.accounts.lastfm.threshold.is_none(),
            "read once, then gone"
        );

        let mut settings = Settings {
            scrobble_threshold: 0.3,
            ..Settings::default()
        };
        settings.accounts.lastfm = serde_json::from_value(serde_json::json!({
            "threshold": 0.8
        }))
        .unwrap();
        seed_threshold(
            &mut settings,
            &serde_json::json!({ "scrobble_threshold": 0.3 }),
        );
        assert_eq!(
            settings.scrobble_threshold, 0.3,
            "the written knob wins over the stale account copy"
        );
    }

    #[test]
    fn the_legacy_switch_seeds_the_shared_one_once() {
        let mut settings = Settings::default();
        settings.accounts.lastfm =
            serde_json::from_value(serde_json::json!({ "scrobbling": false })).unwrap();
        seed_scrobbling(&mut settings, &serde_json::json!({}));
        assert!(!settings.scrobbling);
        assert!(
            settings.accounts.lastfm.scrobbling.is_none(),
            "read once, then gone"
        );

        let mut settings = Settings::default();
        settings.accounts.lastfm =
            serde_json::from_value(serde_json::json!({ "scrobbling": false })).unwrap();
        seed_scrobbling(&mut settings, &serde_json::json!({ "scrobbling": true }));
        assert!(
            settings.scrobbling,
            "the written switch wins over the stale account copy"
        );
    }

    #[test]
    fn look_file_roundtrips() {
        let mut src = dressed().look;
        src.active_layout = Some("one".into());
        src.layout = Some(serde_json::json!({ "dock": "live" }));
        src.layout_edits.insert(
            "two".into(),
            LayoutEdit {
                dump: serde_json::json!({ "k": "edited" }),
                size: None,
            },
        );

        let json = serde_json::to_string_pretty(&src).unwrap();
        let back: LookState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.active_layout.as_deref(), Some("one"));
        assert_eq!(back.layout, src.layout);
        assert_eq!(back.layout_edits.len(), 1);
        assert_eq!(back.bundle.layouts.len(), 1);
        assert_eq!(back.bundle.appearance.surface_opacity, 0.5);
        assert!(back.bundle.appearance.rating_style == RatingStyle::Numeric);
        assert_eq!(
            back.bundle.palette_dark.get("accent").map(String::as_str),
            Some("#336699")
        );
    }

    #[test]
    fn legacy_settings_yields_its_look() {
        let json = serde_json::json!({
            "volume": 0.8,
            "library_roots": ["/music"],
            "layout": { "dock": "live" },
            "active_layout": "one",
            "layout_edits": { "two": { "dump": { "k": "edited" } } },
            "layouts": [{ "name": "one", "dump": { "k": "v" } }],
            "primary_layout": "one",
            "mini_layout": "small",
            "palette_dark": { "accent": "#336699" },
            "palette_light": { "accent": "#663399" },
            "surface_opacity": 0.5,
            "frame": { "margin": 4.0, "padding": 8.0, "rounding": 12.0, "border": 1.0 },
            "seams": false,
            "art_theming": true,
            "rating_style": "numeric",
            "hide_menubar": true,
            "os_decorations": false,
        });

        let look = LookState::from_legacy(&json);
        assert_eq!(look.active_layout.as_deref(), Some("one"));
        assert!(look.layout.is_some());
        assert_eq!(look.layout_edits.len(), 1);
        assert_eq!(look.bundle.layouts.len(), 1);
        assert_eq!(look.bundle.primary_layout.as_deref(), Some("one"));
        assert_eq!(look.bundle.mini_layout.as_deref(), Some("small"));
        assert_eq!(
            look.bundle.palette_dark.get("accent").map(String::as_str),
            Some("#336699")
        );
        assert_eq!(
            look.bundle.palette_light.get("accent").map(String::as_str),
            Some("#663399")
        );
        let a = &look.bundle.appearance;
        assert_eq!(a.surface_opacity, 0.5);
        assert_eq!(a.frame.rounding, 12.0);
        assert!(!a.seams);
        assert!(a.art_theming);
        assert!(a.rating_style == RatingStyle::Numeric);
        assert!(a.hide_menubar);
        assert!(!a.os_decorations);
        assert!(look.bundle.name.is_empty());
    }

    #[test]
    fn legacy_look_from_nothing_is_the_default() {
        let look = LookState::from_legacy(&serde_json::Value::Null);
        assert!(look.layout.is_none());
        assert!(look.bundle.layouts.is_empty());
        assert_eq!(look.bundle.version, WORKSPACE_VERSION);
        assert_eq!(
            look.bundle.appearance.surface_opacity,
            AppearanceBundle::default().surface_opacity
        );
    }

    #[test]
    fn settings_deserialize_tolerates_drift() {
        let json = r#"{ "fold_case": true, "some_future_knob": 42, "quit_to_tray": true }"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert!(s.fold_case);
        assert!(s.quit_to_tray);
        assert!(s.watch_library);
        assert!(s.theme == Theme::default());
        // Design mode defaults on: older files lack the key, and those
        // installs keep the editing controls they had.
        assert!(s.design_mode);

        let session: SessionState = serde_json::from_str(r#"{ "muted": true }"#).unwrap();
        assert!(session.muted);
        assert_eq!(session.volume, SessionState::default().volume);
        assert!(session.loop_mode() == LoopMode::Off);
    }

    #[test]
    fn loop_mode_wire_names_round_trip() {
        let mut s = SessionState::default();
        for (mode, wire) in [
            (LoopMode::Off, "off"),
            (LoopMode::All, "all"),
            (LoopMode::One, "one"),
        ] {
            s.set_loop_mode(mode);
            assert_eq!(s.loop_mode, wire);
            assert!(s.loop_mode() == mode);
        }
        s.loop_mode = "garbage".into();
        assert!(s.loop_mode() == LoopMode::Off);
    }

    #[test]
    fn legacy_settings_yields_the_plain_shards() {
        let json = serde_json::json!({
            "volume": 0.3,
            "muted": true,
            "loop_mode": "all",
            "shuffle": true,
            "last_scan": 12345,
            "update_cache": { "checked_at": 99, "latest": "1.9.0", "url": "https://x" },
            "lastfm": { "username": "zealsprince", "session_key": "sk", "threshold": 0.8 },
            "discord": { "enabled": true },
            "window": { "x": 1.0, "y": 2.0, "width": 800.0, "height": 600.0, "maximized": true },
            "stats_window": { "width": 500.0, "height": 400.0, "range": "year" },
            "console_window": { "width": 700.0, "height": 300.0 },
            "panel_settings_window": { "width": 640.0, "height": 480.0 },
            "settings_window": { "width": 900.0, "height": 700.0 },
            "queue_view": { "columns": ["title"] },
        });

        let session: SessionState = from_legacy(&json);
        assert_eq!(session.volume, 0.3);
        assert!(session.muted);
        assert!(session.loop_mode() == LoopMode::All);
        assert!(session.shuffle);
        assert_eq!(session.last_scan, 12345);
        assert!(session.update_cache.is_some());

        let mut accounts: AccountsState = from_legacy(&json);
        accounts.lastfm.fold_legacy_session();
        assert_eq!(accounts.lastfm.username("any-key"), "zealsprince");
        assert_eq!(accounts.lastfm.threshold, Some(0.8));
        assert!(accounts.discord.enabled);

        let windows: WindowsState = from_legacy(&json);
        assert_eq!(windows.main.map(|w| w.width), Some(800.0));
        assert_eq!(windows.stats.map(|s| s.range), Some("year".to_string()));
        assert_eq!(windows.console.map(|s| s.width), Some(700.0));
        assert_eq!(windows.panel_settings.map(|s| s.width), Some(640.0));
        assert_eq!(windows.settings.map(|s| s.width), Some(900.0));
        assert!(windows.queue_view.is_some());
    }

    /// A missing nested field must not cost the whole shard.
    #[test]
    fn a_short_sub_object_costs_only_itself() {
        let json = serde_json::json!({
            "volume": 0.3,
            "shuffle": true,
            "last_scan": 12345,
            "update_cache": { "checked_at": 99, "latest": "1.9.0" },
        });
        let session: SessionState = from_legacy(&json);
        assert_eq!(session.volume, 0.3);
        assert!(session.shuffle);
        assert_eq!(session.last_scan, 12345);
        assert_eq!(
            session.update_cache.map(|c| c.latest),
            Some("1.9.0".to_string())
        );

        let json = serde_json::json!({
            "window": { "x": 1.0, "y": 2.0, "width": 800.0, "height": 600.0 },
        });
        let windows: WindowsState = from_legacy(&json);
        assert_eq!(windows.main.map(|w| w.height), Some(600.0));
    }

    /// Without the lenient list one bad preset resets all of `workspace.json`.
    #[test]
    fn a_broken_preset_costs_only_that_preset() {
        let json = serde_json::json!({
            "layouts": [
                { "name": "good", "dump": { "k": "v" } },
                { "name": "broken" },
                { "name": "also good", "dump": { "k": "v2" } },
            ],
            "primary_layout": "good",
            "palette_dark": { "accent": "#336699" },
        });
        let bundle: WorkspaceBundle = serde_json::from_value(json).unwrap();
        let names: Vec<&str> = bundle.layouts.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["good", "also good"]);
        assert_eq!(bundle.primary_layout.as_deref(), Some("good"));
        assert_eq!(
            bundle.palette_dark.get("accent").map(String::as_str),
            Some("#336699")
        );
    }

    #[test]
    fn a_broken_working_copy_costs_only_that_copy() {
        let json = serde_json::json!({
            "bundle": {
                "signals": [
                    { "id": 1, "name": "Kick" },
                    { "source": { "nonsense": true } },
                ],
            },
            "layout_edits": {
                "good": { "dump": { "k": "v" } },
                "broken": { "size": { "width": 100.0, "height": 100.0 } },
            },
            "active_layout": "good",
        });
        let look: LookState = serde_json::from_value(json).unwrap();
        assert_eq!(look.layout_edits.len(), 1);
        assert!(look.layout_edits.contains_key("good"));
        assert_eq!(look.bundle.signals.len(), 1);
        assert_eq!(look.bundle.signals[0].name, "Kick");
        assert_eq!(look.active_layout.as_deref(), Some("good"));
    }

    #[test]
    fn approved_shaders_ride_the_session_shard() {
        let mut session = SessionState::default();
        assert!(session.approved_shaders.is_empty());
        let written = serde_json::to_value(&session).expect("dump");
        assert!(
            written.get("approved_shaders").is_none(),
            "an empty list writes no key"
        );

        session.approved_shaders.insert("beef".to_string());
        session.approved_shaders.insert("cafe".to_string());
        let written = serde_json::to_value(&session).expect("dump");
        let read: SessionState = serde_json::from_value(written.clone()).expect("read back");
        assert!(read.approved_shaders.contains("beef"));
        assert!(read.approved_shaders.contains("cafe"));
        assert_eq!(read.approved_shaders.len(), 2);
        // Sorted, so approval order doesn't rewrite the file.
        assert_eq!(
            written["approved_shaders"],
            serde_json::json!(["beef", "cafe"])
        );

        let older: SessionState =
            serde_json::from_value(serde_json::json!({ "volume": 0.4 })).expect("read");
        assert!(older.approved_shaders.is_empty());

        // The trust list must never travel in a shared bundle.
        let bundle = serde_json::to_value(WorkspaceBundle::default()).expect("dump");
        assert!(bundle.get("approved_shaders").is_none());
    }

    #[test]
    fn the_approved_list_is_a_set() {
        let print = "0123456789abcdef-not-a-real-hash";
        assert!(!shader_approved(print));
        assert!(note_approved(print), "the first approval is news");
        assert!(shader_approved(print));
        assert!(
            !note_approved(print),
            "the second is not, so nothing writes"
        );
        forget_approved(print);
        assert!(!shader_approved(print));
    }

    /// The cursor indexes the entries, so a bad entry fails the whole queue
    /// rather than resume the wrong track.
    #[test]
    fn a_broken_queue_costs_only_the_queue() {
        let json = serde_json::json!({
            "volume": 0.4,
            "loop_mode": "all",
            "shuffle": true,
            "last_scan": 12345,
            "last_queue": { "entries": [{ "id": 1, "explicit": false }, { "id": "not a number" }] },
        });
        let session: SessionState = serde_json::from_value(json).unwrap();
        assert!(session.last_queue.is_none());
        assert_eq!(session.volume, 0.4);
        assert!(session.loop_mode() == LoopMode::All);
        assert!(session.shuffle);
        assert_eq!(session.last_scan, 12345);
    }

    #[test]
    fn a_saved_queue_round_trips_its_subs() {
        let state = SessionState {
            last_track: Some(LastTrack {
                id: 7,
                sub: 3,
                position_secs: 12.5,
            }),
            last_queue: Some(QueueState {
                entries: vec![
                    QueuedTrack {
                        id: 7,
                        sub: 3,
                        explicit: false,
                    },
                    QueuedTrack {
                        id: 8,
                        sub: 4,
                        explicit: true,
                    },
                    QueuedTrack {
                        id: 9,
                        sub: 0,
                        explicit: false,
                    },
                ],
                cursor: 1,
                position_secs: 12.5,
            }),
            ..SessionState::default()
        };
        let text = serde_json::to_string(&state).unwrap();
        let back: SessionState = serde_json::from_str(&text).unwrap();
        let queue = back.last_queue.expect("the queue survives the round trip");
        let subs: Vec<u16> = queue.entries.iter().map(|e| e.sub).collect();
        assert_eq!(subs, [3, 4, 0]);
        assert_eq!(queue.cursor, 1);
        assert!(queue.entries[1].explicit);
        let last = back.last_track.expect("the single-track fallback too");
        assert_eq!((last.id, last.sub), (7, 3));

        let json = serde_json::json!({
            "last_track": { "id": 7, "position_secs": 12.5 },
            "last_queue": {
                "entries": [{ "id": 7, "explicit": false }, { "id": 8, "explicit": true }],
                "cursor": 1,
            },
        });
        let old: SessionState = serde_json::from_value(json).unwrap();
        let queue = old
            .last_queue
            .expect("an entry without a sub still reads, so the queue survives");
        assert_eq!(queue.entries.len(), 2);
        assert!(
            queue.entries.iter().all(|e| e.sub == 0),
            "a missing sub reads as a plain file"
        );
        assert_eq!(old.last_track.map(|t| t.sub), Some(0));
    }

    /// Failing instead would cost the whole session shard, `last_scan` and
    /// so a full rescan included, over one word a newer build wrote.
    #[test]
    fn an_unknown_shuffle_mode_costs_only_the_mode() {
        let json = serde_json::json!({
            "volume": 0.4,
            "muted": true,
            "loop_mode": "all",
            "shuffle": true,
            "shuffle_mode": "genre",
            "last_scan": 12345,
            "last_queue": { "entries": [{ "id": 1, "explicit": false }], "cursor": 0 },
        });
        let session: SessionState = serde_json::from_value(json).unwrap();
        assert_eq!(session.shuffle_mode, ShuffleMode::Random);
        assert_eq!(session.volume, 0.4);
        assert!(session.muted);
        assert!(session.loop_mode() == LoopMode::All);
        assert!(session.shuffle);
        assert_eq!(session.last_scan, 12345);
        assert!(session.last_queue.is_some());

        let session: SessionState =
            serde_json::from_value(serde_json::json!({ "shuffle_mode": "similar" })).unwrap();
        assert_eq!(session.shuffle_mode, ShuffleMode::Similar);
        let session: SessionState =
            serde_json::from_value(serde_json::json!({ "shuffle_mode": 7 })).unwrap();
        assert_eq!(session.shuffle_mode, ShuffleMode::Random);
    }

    #[test]
    fn an_unknown_enum_word_costs_only_its_field() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "theme": "midnight",
            "library_roots": ["/music"],
            "eq": { "enabled": true, "analyzer": "spectrogram" },
            "replay_gain": { "mode": "loudest", "save": "cloud", "preamp_db": 3.0 },
        }))
        .unwrap();
        assert!(settings.theme == Theme::default());
        assert_eq!(settings.library_roots, vec![PathBuf::from("/music")]);
        assert!(settings.eq.enabled);
        assert_eq!(settings.eq.analyzer, AnalyzerStyle::default());
        assert_eq!(settings.replay_gain.mode, GainModeSetting::default());
        assert_eq!(settings.replay_gain.save, ReplayGainSave::default());
        assert_eq!(settings.replay_gain.preamp_db, 3.0);

        let accounts: AccountsState = serde_json::from_value(serde_json::json!({
            "lastfm": { "sessions": { "api-key": { "key": "a-real-secret" } } },
            "providers": { "lyrics_save": "somewhere-else", "musicbrainz": false },
        }))
        .unwrap();
        assert!(accounts.providers.lyrics_save == LyricsSave::default());
        assert!(!accounts.providers.musicbrainz);
        assert_eq!(
            accounts.lastfm.session("api-key").map(|s| s.key.as_str()),
            Some("a-real-secret")
        );
        assert!(accounts.listenbrainz.token.is_empty());
        assert!(accounts.listenbrainz.username.is_none());

        let look: LookState = serde_json::from_value(serde_json::json!({
            "bundle": {
                "appearance": { "rating_style": "hearts", "rating_dots": true },
                "palette_dark": { "accent": "#336699" },
            },
        }))
        .unwrap();
        assert!(look.bundle.appearance.rating_style == RatingStyle::default());
        assert!(look.bundle.appearance.rating_dots);
        assert_eq!(
            look.bundle.palette_dark.get("accent").map(String::as_str),
            Some("#336699")
        );
    }

    #[test]
    fn the_acoustid_provider_fields_round_trip_and_default() {
        let providers = Providers {
            acoustid: false,
            acoustid_key: "a-registered-application-key".to_string(),
            ..Providers::default()
        };
        let text = serde_json::to_string(&providers).unwrap();
        let read: Providers = serde_json::from_str(&text).unwrap();
        assert!(!read.acoustid);
        assert_eq!(read.acoustid_key, "a-registered-application-key");
        assert!(read.musicbrainz);

        let old: Providers = serde_json::from_value(serde_json::json!({
            "lrclib": true,
            "musicbrainz": false,
        }))
        .unwrap();
        assert!(old.acoustid);
        assert!(old.acoustid_key.is_empty());
        assert!(!old.musicbrainz);
    }

    #[test]
    fn the_discord_card_lines_round_trip_and_default() {
        let discord = DiscordSettings {
            enabled: true,
            first_line: "%title%".to_string(),
            second_line: String::new(),
            hover_line: "%genre%".to_string(),
            status_line: DiscordStatusLine::Second,
            ..DiscordSettings::default()
        };
        let text = serde_json::to_string(&discord).unwrap();
        let read: DiscordSettings = serde_json::from_str(&text).unwrap();
        assert_eq!(read.first_line, "%title%");
        assert!(read.second_line.is_empty());
        assert_eq!(read.hover_line, "%genre%");
        assert_eq!(read.status_line, DiscordStatusLine::Second);

        let old: DiscordSettings = serde_json::from_value(serde_json::json!({
            "enabled": true,
            "show_lastfm_button": false,
        }))
        .unwrap();
        assert_eq!(old.first_line, DEFAULT_PRESENCE_FIRST_LINE);
        assert_eq!(old.second_line, DEFAULT_PRESENCE_SECOND_LINE);
        assert_eq!(old.hover_line, DEFAULT_PRESENCE_HOVER);
        assert_eq!(old.status_line, DiscordStatusLine::First);
        assert!(old.enabled);
        assert!(!old.show_lastfm_button);

        let newer: DiscordSettings = serde_json::from_value(serde_json::json!({
            "enabled": true,
            "status_line": "third",
        }))
        .unwrap();
        assert_eq!(newer.status_line, DiscordStatusLine::First);
        assert!(newer.enabled);
    }

    #[test]
    fn a_broken_window_shape_costs_only_that_window() {
        let json = serde_json::json!({
            "main": { "x": 1.0, "y": 2.0, "width": 800.0, "height": 600.0, "maximized": false },
            "stats": { "width": "wide" },
            "console": { "width": 700.0, "height": 300.0 },
        });
        let windows: WindowsState = serde_json::from_value(json).unwrap();
        assert!(windows.stats.is_none());
        assert_eq!(windows.main.map(|w| w.width), Some(800.0));
        assert_eq!(windows.console.map(|s| s.width), Some(700.0));
    }

    /// Without this, a retrained checkpoint's vectors land under the old id.
    #[test]
    fn a_rewritten_weights_file_stamps_differently() {
        let dir = std::env::temp_dir().join(format!("rox-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("weights.safetensors");
        std::fs::write(&path, b"one checkpoint").unwrap();
        let first = file_stamp(&path).unwrap();
        std::fs::write(&path, b"a different checkpoint").unwrap();
        assert_ne!(file_stamp(&path), Some(first));
        assert_eq!(file_stamp(&dir), None);
        assert_eq!(file_stamp(&dir.join("gone")), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Right once: a pre-split file serializes to the same bytes after a
    /// no-op edit, and without the force its flat keys, credentials among
    /// them, would never be stripped.
    #[test]
    fn a_migrated_load_rewrites_an_unmoved_file() {
        let dir = std::env::temp_dir().join(format!("rox-shard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shard.json");
        std::fs::write(&path, r#"{"stale": true}"#).unwrap();
        let print = serde_json::to_string(&SessionState::default()).ok();

        write_shard(
            path.clone(),
            "shard",
            &print,
            &print,
            false,
            &SessionState::default(),
        );
        assert!(std::fs::read_to_string(&path).unwrap().contains("stale"));

        write_shard(
            path.clone(),
            "shard",
            &print,
            &print,
            true,
            &SessionState::default(),
        );
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(!written.contains("stale"));
        assert!(written.contains("volume"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A second copy in the machine file would shadow what an apply brings.
    #[test]
    fn the_backdrop_visual_look_lives_in_the_bundle_not_the_machine_file() {
        let config = BackdropVisualConfig {
            enabled: true,
            strength: 0.8,
            color: MilkdropColor::Cover,
            locked: false,
            ..BackdropVisualConfig::default()
        };
        let machine = serde_json::to_value(&config).expect("serializes");
        assert!(machine.get("enabled").is_none());
        assert!(machine.get("strength").is_none());
        assert!(machine.get("color").is_none());
        assert_eq!(machine["locked"], false);

        let look = config.look();
        let bundled = serde_json::to_value(look).expect("serializes");
        assert_eq!(bundled["enabled"], true);
        assert_eq!(bundled["color"], "cover");

        let read: BackdropVisualConfig = serde_json::from_value(machine).expect("reads");
        assert!(!read.enabled, "the machine file says nothing about it");
        let merged = read.with_look(&look);
        assert!(merged.enabled);
        assert_eq!(merged.strength, 0.8);
        assert_eq!(merged.color, MilkdropColor::Cover);
        assert!(!merged.locked, "the machine's own field survives the merge");
    }
}
