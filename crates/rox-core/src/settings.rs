//! Persisted app settings, in the app's data directory next to the library
//! database. Three pieces: `settings.json` holds the machine state (playback,
//! library folders, accounts, window frames), `workspace.json` holds the look
//! the app is wearing, and `workspaces/` holds the saved workspaces, one file
//! each. Writers each own a few fields (the player its playback state, the
//! workspace its window and layout) and write through [`Settings::update`],
//! which reloads first so one writer's save never reverts another's fields to
//! what they were at startup.
//!
//! The split keeps `settings.json` small enough to read and hand-edit: the
//! dock dumps and palettes that dwarfed it now sit in their own files, and a
//! saved workspace on disk is already an exported one.
//!
//! `layouts` here is the named dock presets, `panel_presets` the named single
//! panels. The settings window and its chrome (`ui`, `window`,
//! `shader_confirm`) sit up in rox, where the widgets are.

pub mod layouts;
pub mod panel_presets;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{LazyLock, OnceLock, RwLock};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use gpui::{px, App, SharedString, WindowAppearance, WindowDecorations};
use serde::{Deserialize, Serialize};

use rox_playback::engine::LoopMode;
use rox_viz::signal::{Route, Signal};

use rox_design::palette::{self, Palette, Sides};

use crate::acoustic;
use crate::continuation;

/// The floor under every rox window. Applying a layout or toggling the
/// mini-player resizes the window to a preset's stored size, and a bad or
/// zero size there used to collapse the window to nothing, so you had to go
/// fish it back out with the window manager. This is the OS-level minimum and
/// the clamp the programmatic resizes run through, never zero.
///
/// Low enough to stay out of the way, because it isn't what usually stops a
/// resize. The dock floors a window at what its layout needs: every panel
/// carries a minimum, a stack adds its children's along its own axis, and
/// that sum is what a drag actually hits. This is the backstop under that,
/// for a layout whose panels have all been set small enough to reach it.
pub const MIN_WINDOW_SIZE: gpui::Size<gpui::Pixels> = gpui::Size {
    width: px(20.),
    height: px(20.),
};

/// Where a pre-split settings file's workspaces go. The bundle handling lives
/// up in rox, so the migration hands each one back through here; startup
/// installs the sink before anything reads a setting.
static WORKSPACE_MIGRATOR: OnceLock<fn(WorkspaceBundle)> = OnceLock::new();

/// Point [`Settings::load`]'s one-shot migration at the workspace writer.
pub fn set_workspace_migrator(migrate: fn(WorkspaceBundle)) {
    let _ = WORKSPACE_MIGRATOR.set(migrate);
}

/// The folder holding the running executable, portable mode's anchor.
/// None when the exe path can't be read, which just leaves portable off.
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()))
}

/// The marker file beside the executable that keeps portable mode on
/// across launches; the Behavior page's toggle creates and removes it.
pub fn portable_marker() -> Option<PathBuf> {
    exe_dir().map(|dir| dir.join("portable"))
}

/// The portable data folder beside the executable. Named rox-data rather
/// than data so it stays recognizable in a folder shared with other apps.
pub fn portable_data_dir() -> Option<PathBuf> {
    exe_dir().map(|dir| dir.join("rox-data"))
}

/// The resolved data root and whether it is the portable one, decided
/// once per process so a mid-run toggle can't split the stores: the
/// `portable` marker beside the executable, or a `--portable` flag for
/// one run, routes everything into rox-data; a flip lands on the next
/// launch. In debug builds `--fresh` overrides both with a wiped scratch
/// folder for testing the first-run experience.
static DATA_DIR: OnceLock<(PathBuf, bool)> = OnceLock::new();

fn resolve_data_dir() -> (PathBuf, bool) {
    // A fresh run routes everything into a scratch folder in the OS temp
    // dir, wiped here (the once-per-process choke point) so each launch
    // lands on a genuine first run: no settings file, so the welcome
    // window shows, and no library or caches. Debug-build aid for the
    // first-time experience (`cargo run -- --fresh`); release builds
    // ignore the flag so it never becomes user-facing surface.
    if cfg!(debug_assertions) && std::env::args().any(|arg| arg == "--fresh") {
        let dir = std::env::temp_dir().join("rox-fresh");
        let _ = std::fs::remove_dir_all(&dir);
        return (dir, false);
    }
    let portable = std::env::args().any(|arg| arg == "--portable")
        || portable_marker().is_some_and(|marker| marker.exists());
    if portable {
        if let Some(dir) = portable_data_dir() {
            return (dir, true);
        }
    }
    let dir = dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rox");
    (dir, false)
}

/// The app's data directory, shared with the library database. Created on
/// first use. Portable runs read rox-data beside the executable instead
/// of the OS data dir.
pub fn data_dir() -> PathBuf {
    let (dir, _) = DATA_DIR.get_or_init(resolve_data_dir);
    let _ = std::fs::create_dir_all(dir);
    dir.clone()
}

/// Whether this run reads the portable folder, however it was asked for.
pub fn portable() -> bool {
    DATA_DIR.get_or_init(resolve_data_dir).1
}

/// Whether the executable's folder takes writes, the portable toggle's
/// gate: install dirs (app bundles, Program Files, /usr/bin) are often
/// read-only, and a directory permission read doesn't answer reliably
/// across platforms, so probe with a real file.
pub fn portable_available() -> bool {
    let Some(dir) = exe_dir() else {
        return false;
    };
    let probe = dir.join(".rox-write-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Whether this launch found no settings file, the welcome window's
/// first-run signal. Recorded once at startup through [`note_first_run`],
/// before anything can write the file.
static FIRST_RUN: AtomicBool = AtomicBool::new(false);

pub fn note_first_run() {
    FIRST_RUN.store(!settings_path().exists(), Ordering::Relaxed);
}

pub fn first_run() -> bool {
    FIRST_RUN.load(Ordering::Relaxed)
}

/// The settings file inside [`data_dir`], public so the settings window
/// can hand the raw file to the system editor. Preferences and the library
/// setup only: the things a person would actually want to read, change, or
/// carry to another machine.
pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

/// The live look's own file: the workspace the app is wearing plus its
/// working state. The dock dumps and palettes that dwarfed everything else
/// live here.
pub fn look_path() -> PathBuf {
    data_dir().join("workspace.json")
}

/// Where the windows sit on this machine: the main frame plus what each
/// auxiliary window remembers. Never worth carrying anywhere, and safe to
/// delete - the windows just reopen at their defaults.
pub fn windows_path() -> PathBuf {
    data_dir().join("windows.json")
}

/// What was playing and where the library stood: the volatile half that
/// changes on every track and would otherwise churn the preferences file.
/// Safe to delete; it all regenerates.
pub fn session_path() -> PathBuf {
    data_dir().join("session.json")
}

/// The account connections and their keys. Its own file so the file people
/// are invited to open and hand around carries no credentials, and a sync
/// setup can leave the secrets behind.
pub fn accounts_path() -> PathBuf {
    data_dir().join("accounts.json")
}

/// The folder the user's saved workspaces live in, one JSON file each. A
/// bundle on disk is already an exported bundle: drop a shared file in here
/// and it joins the list, delete one and it's gone.
pub fn workspaces_dir() -> PathBuf {
    data_dir().join("workspaces")
}

/// The folder the ejected shaders live in, one subfolder per workspace and
/// one `.wgsl` per pool entry. Ejecting is how a shader that arrived inside
/// a bundle gets a file an editor can open, and the file is what hot reload
/// watches from then on. Nothing is created here; the first eject makes the
/// folders, the same rule the lyrics and artist stores keep.
pub fn shaders_dir() -> PathBuf {
    data_dir().join("shaders")
}

/// Where a workspace's shader ejects to. Both halves of the name double as
/// path components, so both go through [`safe_file_stem`]; a look that was
/// never saved under a name (the live one you're editing) lands under
/// `_local`. A workspace someone actually calls "_local" shares that folder,
/// which is a name collision like any other here, and the re-link only takes
/// a file whose contents still hash to the entry's, so the worst it costs is
/// a bookmark that doesn't attach.
pub fn shader_eject_path(workspace: &str, shader: &str) -> PathBuf {
    shader_eject_path_in(&shaders_dir(), workspace, shader)
}

/// The eject path under a given root. What the tests and the re-link up in
/// rox exercise without writing into the folder the running app ejects to.
pub fn shader_eject_path_in(root: &Path, workspace: &str, shader: &str) -> PathBuf {
    root.join(safe_file_stem(workspace, "_local"))
        .join(format!("{}.wgsl", safe_file_stem(shader, "shader")))
}

/// A name as a file or folder name. Names double as filenames all over the
/// data directory, so anything that can't be one is stripped: separators,
/// the characters Windows refuses, and control characters all fold to
/// spaces, then the result is trimmed of space and of the leading dots that
/// would hide the file. A name of pure punctuation empties out and lands on
/// `fallback`.
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

/// Write pretty JSON through a sibling temp file, then rename over the real
/// one. A crash mid-write can't truncate a file and take every layout,
/// palette, and the Last.fm session down with it; rename is atomic within the
/// same directory. Failures log under `what` and move on: losing a write is
/// not worth interrupting playback for.
pub fn write_json<T: Serialize>(path: &Path, value: &T, what: &str) -> bool {
    let text = match serde_json::to_string_pretty(value) {
        Ok(text) => text,
        // A non-finite f32 would fail here; log and keep the old file rather
        // than panic the whole app mid-playback.
        Err(e) => {
            log::warn!("{what}: serializing: {e}");
            return false;
        }
    };
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            log::warn!("{what}: creating {}: {e}", dir.display());
            return false;
        }
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

/// Deserializers that keep a file readable when one piece of it isn't.
///
/// Serde's default is all or nothing: a preset whose dump went missing fails
/// the list, which fails the look, which resets a whole file to defaults over
/// one bad entry. These narrow that blast radius to the piece that's actually
/// broken. A collection drops the entries that don't parse and keeps the rest;
/// an optional field reads as None, a defaulted one as its default. All three
/// say what they dropped, since a silent one is a preset or a queue vanishing
/// with no thread back to why.
///
/// Which of the three a field takes is not a style choice. A list of presets is
/// independent, so dropping one costs one preset. A queue's `cursor` indexes
/// its `entries`, so dropping an entry shifts the cursor and resumes the wrong
/// track: that one has to fail whole, as an option, or not at all. A closed set
/// of words, a mode or a style or a destination, takes the default: a spelling
/// a newer build wrote is a word this build doesn't know rather than damage,
/// and refusing it would cost the whole shard.
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

/// Each file's serialized contents at one moment, so a write can tell which
/// ones an edit actually touched.
#[derive(PartialEq)]
struct Shards {
    core: Option<String>,
    look: Option<String>,
    windows: Option<String>,
    session: Option<String>,
    accounts: Option<String>,
}

/// Read one shard file, or fall back to reading it out of a pre-split
/// `settings.json` where its fields sat flat beside everything else. A file
/// that no longer parses resets to defaults rather than blocking start, and
/// never falls through to the legacy read: the shard's contents are gone
/// either way, and a stale copy would only resurrect an older version of them.
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

/// Read a shard straight out of a pre-split map. Every field kept its name
/// through the move, so this is the whole migration for three of the four
/// shards; the look needs its own because its appearance knobs went from flat
/// siblings to a nested object.
///
/// A map this can't read costs the whole shard, so it says so rather than
/// quietly handing back defaults: that's an upgrade losing someone's playback
/// state or Last.fm session, and a log line is the only thread back to why.
fn from_legacy<T: Default + serde::de::DeserializeOwned>(value: &serde_json::Value) -> T {
    if value.is_null() {
        return T::default();
    }
    serde_json::from_value(value.clone()).unwrap_or_else(|e| {
        log::warn!("settings: reading the old file's contents: {e}");
        T::default()
    })
}

/// Write one shard when the edit moved it, or when its file isn't there yet.
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

/// The preferences and the library setup, `settings.json`'s own contents,
/// plus the four states that live in files of their own. Unknown fields are
/// dropped on load and missing ones take defaults, so every file survives
/// version drift in both directions. The shards below are skipped here and
/// written separately; this struct holds them so callers still see one
/// settings object and go through one [`Settings::update`].
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The look the app is wearing: the live workspace bundle plus the dock
    /// state it's working on. Persisted to [`look_path`].
    #[serde(skip)]
    pub look: LookState,
    /// Where this machine's windows sit. Persisted to [`windows_path`].
    #[serde(skip)]
    pub windows: WindowsState,
    /// What was playing and where the library stood. Persisted to
    /// [`session_path`].
    #[serde(skip)]
    pub session: SessionState,
    /// The account connections. Persisted to [`accounts_path`].
    #[serde(skip)]
    pub accounts: AccountsState,
    /// Whether this was read out of a pre-split file. The shards are all
    /// missing in that case so they write themselves, but this file is
    /// already on disk holding the old flat shape, and a no-op edit
    /// serializes to the same bytes it would have anyway. Without this the
    /// stale keys, credentials included, would sit there forever.
    #[serde(skip)]
    migrated: bool,
    /// The folders the library scans, in the order they were added. Empty
    /// until one has been opened.
    pub library_roots: Vec<PathBuf>,
    /// The single folder `library_roots` replaced. Read once on load to
    /// seed the list, never written back.
    #[serde(skip_serializing)]
    library_root: Option<PathBuf>,
    /// Whether the library watches its roots for filesystem changes and
    /// folds adds, edits, and deletes in without a manual rescan. On by
    /// default; the settings toggle turns it off for network mounts or when
    /// the watch load is not wanted.
    pub watch_library: bool,
    /// Whether library values differing only by case count as one: Rock
    /// and rock become the same genre, artist, album artist, and album,
    /// shown under the casing most tracks carry. Off keeps values exact,
    /// today's behavior; flipping it reloads the projection.
    pub fold_case: bool,
    /// Whether commas and slashes split genre lists alongside the
    /// semicolon that always does: "Dubstep, Trap" and "Drum & Bass /
    /// Neurofunk" count each value on their own. On by default; off for
    /// libraries whose slashes name single genres. Flipping it reloads
    /// the projection.
    pub split_genre_compounds: bool,
    /// The theme pick: which of the two user palettes renders, with
    /// System following the OS's light/dark preference live.
    #[serde(deserialize_with = "lenient::or_default")]
    pub theme: Theme,
    /// The app-wide text size in px, the rem every window's rem-based text
    /// scales from. Clamped to the palette's shared range on apply; 16 is
    /// the stock size the app has always drawn at.
    pub app_font_size: f32,
    /// The active icon pack by name, a folder of SVGs under the packs dir
    /// that overrides the built-in icons. None uses the built-in set, as
    /// does a name whose folder is gone. Applied at startup; a switch lands
    /// on the next launch, since rendered icons keep their cached tiles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_pack: Option<String>,
    /// Whether launch loads the last playing track back up, paused where
    /// it left off. The track below is written either way; this only
    /// gates the restore.
    pub restore_last_track: bool,
    /// The equalizer's curve and whether it's on, the Audio page's
    /// Equalizer section. A preference rather than session state: it's a
    /// tone choice, and it travels with a copied settings file the way the
    /// rest of this file does.
    pub eq: EqSettings,
    /// How long a crossfade runs at a track boundary, in seconds. Zero is
    /// off and every boundary stays the gapless splice (ADR 19). Tracks
    /// that belong to the same album never fade whatever this says: the
    /// fade is for the cut between unrelated music that shuffle and
    /// skipping make, not for a boundary an engineer meant to be seamless.
    pub crossfade_secs: f32,
    /// The length a switched-off crossfade comes back at. The field above
    /// says off with a zero, so it has nowhere to keep the number a toggle
    /// would restore; this holds the last length that was actually set, and
    /// the transport's crossfade button reads it when it turns the fade back
    /// on. Never zero: with nothing ever set it stands at
    /// [`DEFAULT_CROSSFADE_SECS`].
    pub crossfade_restore_secs: f32,
    /// Whether the fade also takes boundaries inside one album, which the
    /// rule above leaves alone. Off by default: a record that runs track
    /// into track was made that way, and fading it is a change to the
    /// record rather than a smoothing of it.
    pub crossfade_albums: bool,
    /// How tagged loudness is levelled, the Audio page's ReplayGain
    /// section.
    pub replay_gain: ReplayGainSettings,
    /// How the samples reach the device, the Audio page's Output section.
    pub output: OutputSettings,
    /// Whether closing the last workspace window leaves the app resident,
    /// music playing, with the tray (Linux) or the dock (macOS) as the way
    /// back in. Off quits, the default. Ignored on Windows until a tray
    /// backend exists there; a headless process would have no way back.
    pub quit_to_tray: bool,
    /// Whether the layout can be edited where it sits: the panel menus'
    /// Add Panel, Rename, Duplicate, Pop Out and Close rows, the controls a
    /// composition host floats over its slots, and the dock's own tab drag
    /// and drop. On by default, since a first look at the app is also the
    /// only place these actions announce themselves. Off, the layout reads
    /// as finished furniture and is still edited from the Workspace page's
    /// tree in Settings. A preference rather than part of the workspace
    /// bundle: it's how someone works, not how a look is built, so applying
    /// a workspace leaves it alone.
    pub design_mode: bool,
    /// Whether panel resizing is reserved for design mode. Off by
    /// default, so the seams stay draggable whatever the mode and a fresh
    /// layout is easy to shape. On, a finished layout only resizes while
    /// design mode is, and a drag near a seam can't nudge it. A
    /// preference like design mode above, not part of the workspace
    /// bundle.
    pub resize_lock: bool,
    /// Whether launch checks GitHub for a newer release, at most once a
    /// day. The About page's toggle flips it; off leaves only the manual
    /// button.
    pub check_updates: bool,
    /// Whether the unfinished work shows: the experimental panels join the
    /// Panels menu and the launcher. Off by default, flipped on the
    /// Development page. A layout that already holds an experimental panel
    /// still restores it either way.
    pub experimental: bool,
    /// Whether anything of rox talks to AI tooling: the MCP surface, and
    /// any LLM-facing feature that comes later (ADR 22). Off by default,
    /// flipped at the top of the Application page, and revealing the MCP and
    /// ML Models pages when on. The built-in acoustic analysis below
    /// stands on its own and never reads this; enablement only layers AI
    /// capability on top.
    pub ai_enabled: bool,
    /// Whether the MCP surface actually answers tool calls. Its own switch
    /// under [`ai_enabled`](Self::ai_enabled): turning AI on reveals the MCP
    /// page but doesn't open the door, and the rox-mcp proxy checks this on
    /// every call, so a flip applies to the next tool use. Off by default.
    pub mcp_enabled: bool,
    /// Whether the library may describe how its tracks sound, the vectors
    /// behind "more like this". Off by default and separate from the panel
    /// switch above: this one costs real decoding time across the whole
    /// library rather than just showing something that was already built.
    /// Flipped on the Library page, which is also where the extractor is
    /// picked and the pass is run from, since all three are about what the
    /// library knows.
    pub acoustic_analysis: bool,
    /// Whether the analysis pass follows the watcher, so files that land
    /// in the library while rox is running get described without anyone
    /// asking. Off by default, [`ReplayGainSettings::auto`]'s stance: a
    /// pass that decodes audio shouldn't start on its own until it's been
    /// agreed to once. Means nothing with the switch above off.
    pub acoustic_auto: bool,
    /// Whether the library may work out what its tracks run at, the tempo
    /// pass behind the BPM column. Off by default, the acoustic switch's
    /// twin in every respect: this one is the feature as well as the
    /// permission, so with it off nothing measures and the column isn't
    /// offered. Flipped on the Library page beside the acoustic rows,
    /// since both are about what the library knows about its audio.
    pub tempo_analysis: bool,
    /// Whether the tempo pass follows the watcher, the acoustic auto
    /// switch's twin. Off by default; means nothing with the switch above
    /// off.
    pub tempo_auto: bool,
    /// How many tracks the analysis pass works on at once. The default
    /// leaves the machine usable while a pass runs behind other work;
    /// someone happy to hand the whole box over for an afternoon raises it
    /// on the prompt that opens before a pass. Clamped to the machine's own
    /// cores when a pass starts, so a settings file carried from a bigger
    /// machine can't oversubscribe a smaller one. A pass already running
    /// keeps the count it started with.
    ///
    /// Lives here rather than on the prompt alone so the last pick is the
    /// next pass's default: someone who settled on two workers shouldn't
    /// have to say so every time.
    pub acoustic_workers: usize,
    /// The same for the ReplayGain measurement pass, which parallelizes by
    /// album. Its own field rather than a shared one because the two passes
    /// don't cost the same thing: analysis is arithmetic start to finish,
    /// while measurement in tags mode spends part of every file writing to
    /// disk, so the counts that suit them differ.
    pub replaygain_workers: usize,
    /// The same for the tempo pass, which parallelizes by track. Its own
    /// field for the same reason the other two have theirs: a tempo
    /// estimate decodes a minute of audio per track and nothing else, so
    /// the count that suits it is neither of theirs.
    pub tempo_workers: usize,
    /// Which model the analysis pass runs and which model's vectors the
    /// similarity queries read, by its catalog id
    /// (rox's embeddings model catalog). Sits next to the switch
    /// above because neither means anything without the other. Vectors from
    /// every model coexist in the database, so switching back and forth
    /// costs nothing already analyzed. A name from a newer build, or one
    /// whose downloaded weights have since been deleted, falls back to the
    /// built-in extractor.
    pub acoustic_model: String,
    /// Which downloadable model the ML Models page is offering, by catalog
    /// id. Distinct from the field above, which is what the library is
    /// actually running: the two differ whenever the extractor switch is
    /// sitting on the built-in sketch, and keeping them apart is what lets
    /// the switch go back to a model without asking which one again.
    pub acoustic_ml_model: String,
    /// A weights file the user pointed rox at, outside the catalog. One at a
    /// time: this is a way to run a checkpoint of your own, not a second
    /// catalog to manage. Its id is derived from the file's hash rather than
    /// chosen, so its vectors can never land in another model's coordinates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acoustic_local_model: Option<LocalModel>,
    /// Where the analysis pass puts the vectors it works out. Read once when
    /// a pass starts, like the ReplayGain destination it mirrors.
    #[serde(deserialize_with = "lenient::or_default")]
    pub acoustic_save: AcousticSave,
    /// The whole-window post-process shader, the Shader settings page's Screen
    /// shader section.
    pub post_shader: PostShaderConfig,
    /// What the convert dialog opens on: the preset it last ran, where it
    /// wrote, how it named the files, and which ffmpeg to spawn.
    pub convert: ConvertSettings,
    /// The chords that have been moved off their defaults, by command id
    /// (rox's keymap registry). Only what differs is written: a command
    /// with no entry here runs the chords it ships with, so a default that
    /// changes in a later build reaches everyone who never touched it.
    ///
    /// An entry holding an empty list is a command bound to nothing on
    /// purpose. That's the state the absent entry can't express, and it's
    /// why unbinding writes the empty list instead of removing the key.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub keymap: BTreeMap<String, Vec<String>>,
}

/// A weights file outside the catalog, as the settings file carries it. The
/// live form is rox's `embeddings::Local`; this is the same pair of values
/// with a serde derive on it.
#[derive(Clone, Serialize, Deserialize)]
pub struct LocalModel {
    /// Where the file is. Absolute, and not copied into the data folder: a
    /// checkpoint someone is iterating on should stay where they're building
    /// it, and rox re-reads it whenever a pass starts.
    pub path: PathBuf,
    /// The name its vectors are stored under, from
    /// rox's `embeddings::local_id`.
    pub id: String,
    /// What the file looked like when that hash was taken, [`file_stamp`]'s
    /// size and mtime. A checkpoint someone is iterating on gets rewritten at
    /// the same path, and the id would then name bytes that are gone, so
    /// [`resolve_acoustic`] checks these before it hands the file to a pass.
    /// Zero in a file written before the stamp existed, which reads as changed
    /// and costs one re-hash.
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub mtime: i64,
}

/// A weights file's size and its mtime in unix seconds, the pair that says
/// whether the bytes behind a hash are still the ones it was taken from. The
/// scan and the peaks cache stamp files the same way. None when the path isn't
/// a readable file, which reads as the checkpoint being gone.
///
/// Seconds, like every other stamp here, so a rewrite inside the same second
/// as the write that was hashed is the one change this can't see.
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

/// Where this machine's windows sit: `windows.json`'s whole contents. Pure
/// machine state, and the one file here that's disposable - delete it and
/// every window reopens at its default shape, nothing else notices.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowsState {
    /// The main window's last frame, restored on open. None until the first
    /// window closes.
    #[serde(alias = "window", deserialize_with = "lenient::option")]
    pub main: Option<WindowState>,
    /// The tag editor's last window size and column widths, restored on
    /// the next open. None until an editor closes.
    #[serde(deserialize_with = "lenient::option")]
    pub tag_editor: Option<TagEditorState>,
    /// The rename dialog's last size and the patterns it applied, restored
    /// on the next open. None until the dialog closes.
    #[serde(deserialize_with = "lenient::option")]
    pub rename_dialog: Option<RenameDialogState>,
    /// The stats window's last size and range pick, restored on the next
    /// open. None until the window closes.
    #[serde(alias = "stats_window", deserialize_with = "lenient::option")]
    pub stats: Option<StatsWindowState>,
    /// The app settings window's last size, restored on the next open.
    /// None until the window closes.
    #[serde(alias = "settings_window", deserialize_with = "lenient::option")]
    pub settings: Option<LayoutSize>,
    /// The console window's last size, restored on the next open. None until
    /// the window closes.
    #[serde(alias = "console_window", deserialize_with = "lenient::option")]
    pub console: Option<LayoutSize>,
    /// The tasks window's last size, restored on the next open. None until
    /// the window closes.
    #[serde(deserialize_with = "lenient::option")]
    pub tasks: Option<LayoutSize>,
    /// The convert dialog's last size, restored on the next open. None until
    /// the dialog closes. What it converts to and where lives in
    /// [`ConvertSettings`], since those are choices rather than machine
    /// state.
    #[serde(deserialize_with = "lenient::option")]
    pub convert_dialog: Option<LayoutSize>,
    /// The embed dialog's last size, restored on the next open. None until
    /// the dialog closes. Nothing else about it is remembered: what it offers
    /// is whatever the library holds at the time, so there is no choice worth
    /// carrying to the next open.
    #[serde(deserialize_with = "lenient::option")]
    pub bake_dialog: Option<LayoutSize>,
    /// The equalizer window's last size, restored on the next open. None
    /// until the window closes. The curve itself lives in `eq`, since it
    /// shapes audio whether or not the window is ever opened.
    #[serde(alias = "eq_window", deserialize_with = "lenient::option")]
    pub eq: Option<LayoutSize>,
    /// The signals window's last size and the fold state of its explainer,
    /// restored on the next open. None until the window closes. The pool it
    /// edits lives in the look bundle, since it travels with a workspace.
    #[serde(deserialize_with = "lenient::option")]
    pub signals: Option<SignalsWindowState>,
    /// The panel settings window's last size, shared across panels and
    /// restored on the next open. None until a window closes.
    #[serde(alias = "panel_settings_window", deserialize_with = "lenient::option")]
    pub panel_settings: Option<LayoutSize>,
    /// The view for the queue window the widget opens (its columns and album
    /// headings), so the modal and popped-out queue come back the way you
    /// left them. A docked queue panel keeps its own view in the layout dump
    /// instead. Kept as raw JSON, like the dock layout, so the file stays
    /// readable when the queue's config schema moves. None until edited.
    pub queue_view: Option<serde_json::Value>,
}

/// What was playing and where the library stood: `session.json`'s whole
/// contents. The volatile half, rewritten as the music moves, kept off the
/// preferences file so a volume nudge doesn't churn it. Disposable like the
/// windows: delete it and playback starts cold and the library rescans.
#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct SessionState {
    /// Linear playback volume, same range the engine clamps to (0 to 2).
    pub volume: f32,
    /// Whether output is muted. The volume above is the level mute returns
    /// to, so muting never loses the setting.
    pub muted: bool,
    /// Loop mode as its wire name: "off", "all", or "one". The engine's
    /// `LoopMode` stays serde-free; convert through the accessors.
    pub loop_mode: String,
    /// Whether playback shuffles: the queue plays in some order other than
    /// front to back. Which order is [`Self::shuffle_mode`]'s business; this
    /// is only whether shuffling happens at all, so turning it off and back
    /// on returns to the mode that was picked rather than a default.
    pub shuffle: bool,
    /// Which order shuffle puts the queue in. Random is what shuffle has
    /// always meant; Similar orders what's coming by how much it sounds like
    /// the playing track, off the acoustic vectors.
    #[serde(deserialize_with = "lenient::or_default")]
    pub shuffle_mode: ShuffleMode,
    /// Which strategy refills the queue when it runs dry (ADR 17). Continue
    /// out of the box: a local player that goes silent mid-flow feels broken,
    /// and Off is here for anyone who disagrees.
    pub continuation: continuation::Mode,
    /// What was playing when the app closed, as a library track id so it
    /// survives path changes, plus where the clock sat. None when nothing
    /// was playing; a stale id degrades to the cold start on restore.
    #[serde(deserialize_with = "lenient::option")]
    pub last_track: Option<LastTrack>,
    /// The whole play queue as it stood at close, restored on the next launch
    /// so Prev/Next and the queue panel come back. Preferred over
    /// [`SessionState::last_track`]; None when nothing was playing or an older
    /// file predates it, when the single-track fallback takes over.
    #[serde(deserialize_with = "lenient::option")]
    pub last_queue: Option<QueueState>,
    /// When the library last reconciled with disk through a full scan, unix
    /// seconds. Launch catches up on edits made while the app was closed by
    /// scanning, but only when this is stale, so a quick restart does not walk
    /// the whole library again. 0 means never, which always catches up. Kept
    /// here rather than beside the library folders: it describes this
    /// machine's disk, so it must not travel with a copied settings file.
    pub last_scan: i64,
    /// The last update check that landed, so the About page shows an answer
    /// without hitting the network and a launch can tell a fresh check from
    /// a recent one. None until the first check.
    #[serde(
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient::option"
    )]
    pub update_cache: Option<UpdateCache>,
    /// What the last acoustic pass measured on this machine, worker-seconds
    /// per track by model id, so the Library page can price Analyze Missing
    /// before it runs: divide by the worker setting, multiply by what's
    /// missing. Per model because the built-in sketch and a network differ
    /// by most of an order of magnitude, and per machine (which is why it
    /// sits in the session file) because a laptop and a desktop do too.
    /// Empty until a pass has run long enough to measure.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub acoustic_pace: HashMap<String, f32>,
    /// The same for ReplayGain measurement: worker-seconds per track the
    /// last pass averaged. Zero until measured.
    #[serde(skip_serializing_if = "is_zero")]
    pub replaygain_pace: f32,
    /// The same for the tempo pass: worker-seconds per track the last pass
    /// averaged. One number rather than a map, unlike the acoustic pace,
    /// because there's no model behind it to key by. Zero until measured.
    #[serde(skip_serializing_if = "is_zero")]
    pub tempo_pace: f32,
    /// The shader sources this machine has agreed to run, hex SHA-256 of the
    /// trimmed WGSL. Panel shaders ride layout dumps and workspace bundles as
    /// inline source, so an imported look arrives carrying somebody else's
    /// code; nothing registers until its hash is in here. Written by a file
    /// pick, a reload, a preset, or the Approve button, never by an apply.
    /// Machine-local for the same reason the window frames are: a trust
    /// decision belongs to the person who made it, so copying a settings file
    /// around must not carry it. Losing the list costs one Approve per
    /// imported shader, which is why it can sit in the disposable file.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub approved_shaders: BTreeSet<String>,
}

/// Serde's skip test for an unmeasured pace.
fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

/// The account connections: `accounts.json`'s whole contents. Split off so
/// the settings file people are pointed at, and might hand to someone or sync
/// between machines, holds no session keys or API secrets. Not disposable -
/// deleting it means connecting everything again.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AccountsState {
    /// The Last.fm connection and scrobbling knobs, the settings window's
    /// Scrobbling page.
    pub lastfm: Lastfm,
    /// The online enrichment providers and their knobs (ADR 14), the
    /// settings window's Providers page.
    pub providers: Providers,
    /// Discord Rich Presence options (enable toggle, timestamps, details).
    pub discord: DiscordSettings,
}

impl Default for SessionState {
    fn default() -> Self {
        SessionState {
            // Full volume, not silence: a derived default would open the app
            // muted-sounding on a fresh install.
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
            acoustic_pace: HashMap::new(),
            replaygain_pace: 0.0,
            tempo_pace: 0.0,
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

/// The order shuffle puts the upcoming queue in.
///
/// Unlike the loop mode above this is a real enum rather than a wire string,
/// because an unknown value has a sensible answer: fall back to Random, which
/// is what shuffle meant before modes existed and what a settings file
/// written by a newer build should degrade to. The fallback rides the field
/// that reads it, through `lenient::or_default`, so any other field holding one
/// of these needs the same read or it goes back to failing its shard.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShuffleMode {
    /// A random order, the shuffle everyone means by the word.
    #[default]
    Random,
    /// Nearest first by sound: what's coming is ordered by how much it
    /// resembles the track playing when the mode was engaged, off the
    /// acoustic vectors. Needs the library analyzed to do anything.
    Similar,
}

impl ShuffleMode {
    /// The label the mode menu shows.
    pub fn label(self) -> &'static str {
        match self {
            ShuffleMode::Random => "Random",
            ShuffleMode::Similar => "Similar",
        }
    }

    /// Every mode in menu order.
    pub const ALL: [ShuffleMode; 2] = [ShuffleMode::Random, ShuffleMode::Similar];
}

/// The theme pick: dark, light, or the OS's own preference. Dark and
/// light name the two user palettes directly; System resolves to one of
/// them against the desktop's light/dark setting and follows it live.
/// System is the default: a fresh install matches the desktop it lands
/// on. The pick is the user's alone - workspace bundles carry no theme,
/// so applying a look never flips it.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Dark,
    Light,
    #[default]
    System,
}

/// The live theme pick, a static like the rating style's: the OS
/// appearance observers read it to decide whether a change re-themes.
/// Seeded at startup, flipped by the settings window and workspace apply.
static THEME: RwLock<Theme> = RwLock::new(Theme::Dark);

/// The last OS appearance reported, [`seed_os_appearance`]'s startup read
/// refreshed by every workspace window's observer. Cached because the
/// platform's own read borrows the whole Wayland client, which panics
/// from inside window construction or event dispatch; the observers hand
/// us the window's already-cached value instead.
static OS_APPEARANCE: RwLock<WindowAppearance> = RwLock::new(WindowAppearance::Light);

pub fn theme() -> Theme {
    *THEME.read().unwrap()
}

/// Flip the live theme and re-resolve which palette renders. Persisting
/// is the caller's, startup seeds from the file through here too.
pub fn set_theme(theme: Theme, cx: &mut App) {
    *THEME.write().unwrap() = theme;
    palette::set_mode(resolve_theme(theme), cx);
}

/// A theme pick resolved to a palette side: System asks the cached OS
/// appearance, which reads Light until a backend (the xdg-desktop-portal
/// on Linux) has reported otherwise.
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

/// Seed the appearance cache from the platform, once at startup before
/// [`set_theme`]: the one place the platform read is safe, since the
/// event loop is not running yet. The portal may answer after this with
/// the true preference; the window observers fold that in and the theme
/// eases over.
pub fn seed_os_appearance(cx: &App) {
    *OS_APPEARANCE.write().unwrap() = cx.window_appearance();
}

/// A window reported its OS appearance, at open and on every change:
/// refresh the cache, and while the theme follows the system re-resolve
/// the palette side. The mode setter dedupes, so windows past the first
/// and no-op reports cost nothing.
pub fn note_os_appearance(appearance: WindowAppearance, cx: &mut App) {
    *OS_APPEARANCE.write().unwrap() = appearance;
    if theme() == Theme::System {
        palette::set_mode(resolve_theme(Theme::System), cx);
    }
}

/// The rating scale: five stars for quick clicks, or a 0-10 number in
/// half steps for finer review scores. Both write the library's one
/// 0-100 value (a star is 20 points, 7.5 is 75), so flipping the style
/// never loses a rating.
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RatingStyle {
    #[default]
    Stars,
    Numeric,
}

/// A landed update check, cached in the settings file. Holds the latest
/// release GitHub reported rather than a yes/no, so the About page derives
/// up-to-date from the running build - a cached "available" turns to
/// up-to-date on its own once the user updates.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateCache {
    /// Unix seconds of the check, for the once-a-day spacing.
    pub checked_at: u64,
    /// The latest release's version, the leading v stripped.
    pub latest: String,
    /// That release's page on GitHub.
    pub url: String,
}

/// The live rating style, a static like the palette's: rating cells read
/// it in render paths where a settings-file load has no place. Seeded at
/// startup, flipped by the settings window.
static RATING_NUMERIC: AtomicBool = AtomicBool::new(false);

pub fn rating_style() -> RatingStyle {
    if RATING_NUMERIC.load(Ordering::Relaxed) {
        RatingStyle::Numeric
    } else {
        RatingStyle::Stars
    }
}

/// Flip the live style and repaint every window: the static sits outside
/// gpui's reactivity, so nothing else would notice. Persisting is the
/// caller's, startup seeds from the file through here too.
pub fn set_rating_style(style: RatingStyle, cx: &mut App) {
    RATING_NUMERIC.store(style == RatingStyle::Numeric, Ordering::Relaxed);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// The live unrated-dots flag, a static beside the style's, read in the
/// same render paths.
static RATING_DOTS: AtomicBool = AtomicBool::new(false);

pub fn rating_dots() -> bool {
    RATING_DOTS.load(Ordering::Relaxed)
}

/// Flip the dots and repaint, the style setter's twin.
pub fn set_rating_dots(on: bool, cx: &mut App) {
    RATING_DOTS.store(on, Ordering::Relaxed);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// The live menubar-hidden flag, a static like the rating style's: the
/// workspace reads it per frame where a settings-file load has no place.
/// Seeded at startup, flipped by the settings window.
static HIDE_MENUBAR: AtomicBool = AtomicBool::new(false);

pub fn hide_menubar() -> bool {
    HIDE_MENUBAR.load(Ordering::Relaxed)
}

/// Flip the live flag and repaint every window: the static sits outside
/// gpui's reactivity, so nothing else would notice. Persisting is the
/// caller's, startup seeds from the file through here too.
pub fn set_hide_menubar(on: bool, cx: &mut App) {
    HIDE_MENUBAR.store(on, Ordering::Relaxed);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// The live case-fold flag, a static like the menubar's: row scans,
/// rollups, and the projection load read it where a settings-file load
/// has no place. Seeded at startup; the settings window flips it and
/// reloads the projection, whose update repaints everything, so the
/// setter needs no refresh of its own.
static FOLD_CASE: AtomicBool = AtomicBool::new(false);

pub fn fold_case() -> bool {
    FOLD_CASE.load(Ordering::Relaxed)
}

pub fn set_fold_case(on: bool) {
    FOLD_CASE.store(on, Ordering::Relaxed);
}

/// The live OS-decorations flag, a static like the menubar's. Seeded at
/// startup, flipped from the Window menu. Only the main workspace
/// windows follow it; child windows always open with the OS chrome.
static OS_DECORATIONS: AtomicBool = AtomicBool::new(true);

pub fn os_decorations() -> bool {
    OS_DECORATIONS.load(Ordering::Relaxed)
}

/// The flag as the decoration mode new workspace windows open with.
pub fn window_decorations() -> WindowDecorations {
    if os_decorations() {
        WindowDecorations::Server
    } else {
        WindowDecorations::Client
    }
}

/// Flip the live flag. Persisting is the caller's, and so is
/// renegotiating the open workspace windows
/// (`workspace::apply_decorations`).
pub fn set_os_decorations(on: bool) {
    OS_DECORATIONS.store(on, Ordering::Relaxed);
}

/// The live resize-border flag, the decorations flag's twin. Windows only:
/// everywhere else the edges of a borderless window already do nothing, so
/// there's no border to take away and the flag never reaches a window.
static RESIZE_BORDER: AtomicBool = AtomicBool::new(true);

pub fn resize_border() -> bool {
    RESIZE_BORDER.load(Ordering::Relaxed)
}

/// Flip the live flag. Persisting is the caller's, and so is pushing it at
/// the open workspace windows (`workspace::apply_resize_border`).
pub fn set_resize_border(on: bool) {
    RESIZE_BORDER.store(on, Ordering::Relaxed);
}

/// The live panel-seams flag lives in the dock crate, where the resize
/// handles render; these wrappers keep the settings surface in one place.
pub fn seams() -> bool {
    rox_dock::resizable::seams()
}

/// Flip the seams and repaint, the rating-dots setter's twin. Persisting
/// is the caller's, startup seeds from the file through here too.
pub fn set_seams(on: bool, cx: &mut App) {
    rox_dock::resizable::set_seams(on);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// The live design-mode flag, kept in the dock crate for the same reason
/// the seams flag is: the tab groups read it per frame and can't reach app
/// settings from there. These wrappers keep the settings surface in one
/// place.
pub fn design_mode() -> bool {
    rox_dock::design_mode()
}

/// Flip design mode and repaint. Every surface that offers a layout edit -
/// the panel menus, the in-panel controls, the dock's own drag and close -
/// reads the flag as it renders, so the repaint is all it takes.
/// Persisting is the caller's, startup seeds from the file through here
/// too.
pub fn set_design_mode(on: bool, cx: &mut App) {
    rox_dock::set_design_mode(on);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// The live resize-lock flag, the design-mode wrapper's twin: the resize
/// handles read the pair per frame from the dock crate's statics.
pub fn resize_lock() -> bool {
    rox_dock::resize_lock()
}

/// Flip the resize lock and repaint, the design-mode setter's shape.
/// Persisting is the caller's, startup seeds from the file through here
/// too.
pub fn set_resize_lock(on: bool, cx: &mut App) {
    rox_dock::set_resize_lock(on);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// The live quit-to-tray flag, a static like the ones above: the window
/// close path reads it where a settings-file load has no place. Seeded at
/// startup, flipped from the Window menu and the Behavior page.
static QUIT_TO_TRAY: AtomicBool = AtomicBool::new(false);

pub fn quit_to_tray() -> bool {
    QUIT_TO_TRAY.load(Ordering::Relaxed)
}

/// Flip the live flag. Persisting is the caller's, and so is reconciling
/// the tray icon (`tray::sync`).
pub fn set_quit_to_tray(on: bool) {
    QUIT_TO_TRAY.store(on, Ordering::Relaxed);
}

/// The live experimental flag, a static like the ones above: the panel
/// catalog is read while building menus, where a settings-file load has no
/// place. Seeded at startup, flipped on the Development page.
static EXPERIMENTAL: AtomicBool = AtomicBool::new(false);

pub fn experimental() -> bool {
    EXPERIMENTAL.load(Ordering::Relaxed)
}

/// Flip the live flag and repaint every window: the static sits outside
/// gpui's reactivity, and the empty window's launcher draws its tiles
/// straight from the catalog. Persisting is the caller's.
pub fn set_experimental(on: bool, cx: &mut App) {
    EXPERIMENTAL.store(on, Ordering::Relaxed);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// The live acoustic-analysis flag, [`EXPERIMENTAL`]'s twin and a static for
/// the same reason: the library's column registry is read while building the
/// header menu, which is no place to load a settings file.
static ACOUSTIC_ANALYSIS: AtomicBool = AtomicBool::new(false);

pub fn acoustic_analysis() -> bool {
    ACOUSTIC_ANALYSIS.load(Ordering::Relaxed)
}

/// Flip the live flag and repaint, so the Similar column appears in and
/// disappears from the column menus without a relaunch. Persisting is the
/// caller's.
pub fn set_acoustic_analysis(on: bool, cx: &mut App) {
    ACOUSTIC_ANALYSIS.store(on, Ordering::Relaxed);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// The live tempo-analysis flag, [`ACOUSTIC_ANALYSIS`]'s twin and a static
/// for the same reason: the BPM column is offered or withheld while the
/// header menu is being built, where a settings load has no place.
static TEMPO_ANALYSIS: AtomicBool = AtomicBool::new(false);

pub fn tempo_analysis() -> bool {
    TEMPO_ANALYSIS.load(Ordering::Relaxed)
}

/// Flip the live flag and repaint, so the BPM column appears in and
/// disappears from the column menus without a relaunch. Persisting is the
/// caller's.
pub fn set_tempo_analysis(on: bool, cx: &mut App) {
    TEMPO_ANALYSIS.store(on, Ordering::Relaxed);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// The live leveling mode, a static for the column registry's reason: the
/// library's Gain column reads it per cell, and the sort behind that column
/// runs where there's no player entity to ask. Seeded at startup, flipped
/// with the setting. The player keeps its own copy, which is the one the
/// engine levels by; this is only what the library draws.
static GAIN_MODE: AtomicU8 = AtomicU8::new(0);

pub fn gain_mode() -> GainModeSetting {
    match GAIN_MODE.load(Ordering::Relaxed) {
        1 => GainModeSetting::Track,
        2 => GainModeSetting::Album,
        _ => GainModeSetting::Off,
    }
}

/// Publish the mode and repaint, so a Gain column follows the pick without
/// a relaunch. Persisting is the caller's, and startup seeds through here.
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

/// Whether the model in use has actually described anything. The switch
/// above only permits the pass; this says it has run, which is the
/// difference between a mode that ranks by sound and one that quietly does
/// nothing.
///
/// A static for the same reason as its neighbours: the transport draws the
/// shuffle button from it on every frame, and a settings load or a database
/// query has no place there. Published by whoever learns the answer, which
/// is the library on a refresh, the analysis pass when it finishes, and the
/// settings window when the extractor changes under it.
static ACOUSTIC_DESCRIBED: AtomicBool = AtomicBool::new(false);

/// Whether ordering by sound can answer anything right now: the feature is
/// switched on and its model has vectors in the table. What every surface
/// that offers Similar is gated on.
pub fn similarity_ready() -> bool {
    acoustic_analysis() && ACOUSTIC_DESCRIBED.load(Ordering::Relaxed)
}

/// Publish whether the model in use has described anything, and repaint: the
/// shuffle button grows and loses its menu on this, and nothing else would
/// notice the answer changing.
pub fn set_acoustic_described(described: bool, cx: &mut App) {
    if ACOUSTIC_DESCRIBED.swap(described, Ordering::Relaxed) == described {
        return;
    }
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// The live app font, a static like the rating style's: window and panel
/// render paths read it where a settings-file load has no place. None
/// follows the platform default. Seeded at startup, changed by the app
/// settings window.
static APP_FONT: RwLock<Option<SharedString>> = RwLock::new(None);

/// The app-wide font family as it currently stands, for the render paths
/// that apply it at a window root and the panels that fall back to it.
pub fn app_font() -> Option<SharedString> {
    APP_FONT.read().unwrap().clone()
}

/// Set the live app font and repaint every window: the static sits outside
/// gpui's reactivity, so nothing else would notice. Persisting is the
/// caller's, startup seeds from the file through here too.
pub fn set_app_font(font: Option<String>, cx: &mut App) {
    *APP_FONT.write().unwrap() = font.map(SharedString::from);
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// The length a crossfade takes when it's switched on without ever having
/// been set: long enough to hear as an overlap rather than a click, short
/// enough that it doesn't eat the end of a song.
pub const DEFAULT_CROSSFADE_SECS: f32 = 4.0;

/// The frame knobs' ceilings, in px: every knob runs from 0 (off) up to
/// its own. Shared by the app defaults' clamp and both settings windows'
/// sliders, so the app-wide and per-panel frames scrub the same range.
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

/// ADR 13's frame knobs lifted to the app: the cell margin, the inner
/// padding, the corner rounding, and the border width, all in px. Margin,
/// padding, and border carry a value per side, written as one number
/// while the four match. These
/// are the defaults every panel inherits; a panel's own [`PanelTheme`]
/// overrides any of them knob for knob. Zero each by default, so a fresh
/// look carries no frame until asked, matching what an unthemed panel drew
/// before the lift.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Frame {
    pub margin: Sides,
    pub padding: Sides,
    pub rounding: f32,
    pub border: Sides,
}

impl Frame {
    /// Every knob off.
    pub const DEFAULT: Frame = Frame {
        margin: Sides::ZERO,
        padding: Sides::ZERO,
        rounding: 0.0,
        border: Sides::ZERO,
    };

    /// The knobs held to their ceilings, a non-finite one reset to zero,
    /// for a hand-edited file.
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

/// The live app-wide frame defaults, a static like the app font's: the
/// themed wrapper reads it as it lays each panel's frame, in a render path
/// where a settings-file load has no place. Seeded at startup, changed by
/// the app settings window.
static FRAME: RwLock<Frame> = RwLock::new(Frame::DEFAULT);

/// The app-wide frame defaults as they currently stand, for the themed
/// wrapper that lays a panel's frame and the app settings sliders.
pub fn app_frame() -> Frame {
    *FRAME.read().unwrap()
}

/// Set the live frame defaults and repaint every window: the static sits
/// outside gpui's reactivity, so nothing else would notice. Persisting is
/// the caller's, startup seeds from the file through here too.
pub fn set_app_frame(frame: Frame, cx: &mut App) {
    *FRAME.write().unwrap() = frame.clamped();
    for window in cx.windows() {
        window.update(cx, |_, window, _| window.refresh()).ok();
    }
}

/// How the quick-play modal draws its result list, the knobs its inline
/// config panel edits. Persisted so the look survives reopening the modal,
/// which the workspace rebuilds each time.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QuickPlayConfig {
    /// Show a cover thumbnail at the left of each result.
    pub show_cover: bool,
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
            show_cover: false,
            show_subtitle: true,
            show_duration: true,
            comfortable: false,
        }
    }
}

/// One image a shader samples: the flat filename an `// @asset name: file`
/// line points at, and the encoded file itself as base64.
///
/// The bytes are canonical for the same reason the source on
/// [`NamedShader`] is. A plate referenced by path imports as a hole in the
/// look on anyone else's machine, so the file rides along inside the
/// bundle, byte for byte as it sat on disk. Encoded rather than raw pixels
/// because that's what eject writes back out and what `image` reads in, and
/// the 1-bit imagery this is for costs almost nothing that way.
///
/// Assets never gate. Approval is over code, and an image the approved code
/// samples can spoil a look but can't run anything, so no fingerprint ever
/// covers one (ADR 23).
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ShaderAsset {
    /// The name the shader declares it under, which is also the name eject
    /// writes beside the `.wgsl`. A flat filename with no separators in it.
    pub file: String,
    /// The encoded image file, base64. A PNG stays a PNG in here.
    pub data: String,
}

impl ShaderAsset {
    /// Take a file's bytes into an entry, ready to travel.
    pub fn from_bytes(file: impl Into<String>, bytes: &[u8]) -> Self {
        ShaderAsset {
            file: file.into(),
            data: BASE64.encode(bytes),
        }
    }

    /// The encoded file back out, for a decoder or for eject to write
    /// straight to disk. The error is base64's own text, so a hand-edited
    /// entry reads out the way a bad shader does.
    pub fn decode(&self) -> Result<Vec<u8>, String> {
        BASE64
            .decode(self.data.as_bytes())
            .map_err(|err| err.to_string())
    }
}

/// One shader in a workspace's pool: a name, the WGSL behind it, and
/// optionally the file it's being edited in.
///
/// The inline source is canonical. It's what compiles and what runs, and
/// it's the only half that survives the trip to another machine, so a
/// bundle that travelled carries working shaders rather than paths into
/// somebody else's home directory.
///
/// The path is a local bookmark: eject a pool entry to a file and the
/// bookmark links the two, so the hot reload watch can pull edits back into
/// the entry while you work. Export scrubs it ([`WorkspaceBundle::scrub_paths`])
/// because it's dead weight anywhere but the machine that wrote it, and a
/// path riding along would only aim a reload at a file that either isn't
/// there or, worse, is somebody else's.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NamedShader {
    /// What the look's panels point at this entry by. Unique within a pool;
    /// the last entry with a name wins if a hand-edited file repeats one.
    pub name: String,
    /// The fragment stage itself: a `fs_user(uv)` definition and whatever it
    /// calls. This is what runs.
    pub source: String,
    /// The working copy this entry was ejected to, for hot reload. None for
    /// an entry that has never been ejected, and None on every entry in an
    /// exported bundle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// The images the source declares with `// @asset`, carried as bytes so
    /// the look lands whole. Empty for every shader that only reads the
    /// screen, which is most of them, and an empty list writes no key.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient::vec"
    )]
    pub assets: Vec<ShaderAsset>,
}

/// The whole-window post-process shader: whether it runs and which WGSL
/// file it reads. The source lives in a file rather than here because the
/// app has no multi-line editor, and a file gives shader authors hot reload
/// with the editor they already have.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PostShaderConfig {
    /// Whether the pass runs at all. Off is exactly today's rendering.
    pub enabled: bool,
    /// The user's WGSL fragment source file, absolute. None until picked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// The fragment stage inline, the same way a panel shader stores it.
    /// Empty is the older behaviour, where the path above is read at
    /// startup; anything else is what actually runs. Storing it here is what
    /// lets the screen shader travel inside a bundle, since a path alone
    /// imports as a dead pass on anyone else's machine.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// A reference into the workspace's shader pool by name. When it's set
    /// and it resolves, the pool's source wins over the inline copy: the
    /// pool is the one place a bundle's author edits a shader that several
    /// surfaces share. A name that resolves to nothing runs nothing, the
    /// same way a route to a signal that's gone reads zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Whether child windows (settings, stats, equalizer, popped-out
    /// panels) wear the shader too. Off shades only the workspace
    /// windows; meaningless while the switch above is off. The confirm
    /// dialog stays bare either way, so a hostile shader can't take the
    /// way out with it.
    pub all_windows: bool,
    /// The signal routes filling the shader's sixteen slots, the same list
    /// a panel's surface shader carries. Empty is the older behaviour and
    /// stays supported rather than migrated: the pool feeds the slots in
    /// its own order, signal i into slot i. Adding one route takes over the
    /// whole feed, so an unrouted slot reads zero from then on.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<Route>,
    /// Hand-set slot values, the twin of the Shader panel config's list:
    /// what a slot reads with nothing feeding it, which is how a screen
    /// shader's named parameters get tuned without a signal in sight. A
    /// route on the same slot wins while it's there; the hand-set value
    /// comes back when it goes. Under the legacy no-routes feed a hand-set
    /// slot is likewise held out of the pool's order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub manual: Vec<(u8, f32)>,
    /// Keep frames coming while the audio is silent, the panel shaders'
    /// switch grown app-wide. Off, a paused player parks the pass on its
    /// last frame and it costs nothing; on, the pass keeps drawing. The
    /// clock only advances with the signal feed either way, so idle frames
    /// track the mouse without the animation creeping forward. A shader
    /// that reads the pointer asks for its own frames while the pointer
    /// still counts for anything, so it follows the cursor with nothing
    /// playing whichever way this sits.
    pub run_when_idle: bool,
}

impl PostShaderConfig {
    /// Whether anybody has set this up at all: it runs, or it points at
    /// something that could. An untouched default answers false, which is
    /// what keeps a screen shader nobody asked for out of an exported
    /// bundle.
    pub fn configured(&self) -> bool {
        self.enabled || !self.source.is_empty() || self.name.is_some() || self.path.is_some()
    }
}

/// The approved shader hashes, cached out of the session file. The gate is
/// read where a shader is about to register, which is a paint path with no
/// business touching the disk, so the file is read once on the first look
/// and every write goes through [`approve_shader`], which keeps the cache
/// and the file in step.
static APPROVED_SHADERS: LazyLock<RwLock<BTreeSet<String>>> =
    LazyLock::new(|| RwLock::new(Settings::load().session.approved_shaders));

/// The hashes of the shaders inside the bundles this build ships in its own
/// assets. Trusted by construction, the same argument the panel side's
/// `builtin()` makes for the presets: they came with the binary, so asking
/// for a second agreement would only be re-confirming the decision that
/// installing rox already made. Seeded once at startup by the app, which is
/// the only thing that can read its own assets, and never persisted: a
/// shipped set that changes with the build has no business outliving it in
/// somebody's session file.
static SHIPPED_SHADERS: LazyLock<RwLock<BTreeSet<String>>> =
    LazyLock::new(|| RwLock::new(BTreeSet::new()));

/// Record the hashes of every shader the build ships, at startup.
pub fn trust_shipped(fingerprints: impl IntoIterator<Item = String>) {
    SHIPPED_SHADERS.write().unwrap().extend(fingerprints);
}

/// Whether this machine has agreed to run the source behind this hash, or
/// never had to because the build ships it.
pub fn shader_approved(fingerprint: &str) -> bool {
    APPROVED_SHADERS.read().unwrap().contains(fingerprint)
        || SHIPPED_SHADERS.read().unwrap().contains(fingerprint)
}

/// Put a hash in the live list, answering whether it wasn't there already.
/// The half of an approval that costs nothing, split out so the gate's tests
/// can exercise it without a settings file underneath them. Everything
/// outside a test approves through [`approve_shader`], which persists.
pub fn note_approved(fingerprint: &str) -> bool {
    APPROVED_SHADERS
        .write()
        .unwrap()
        .insert(fingerprint.to_string())
}

/// Record a source as approved, here and on disk. Idempotent: approving a
/// hash the list already holds writes nothing, so a reload landing the same
/// text twice doesn't touch the file.
pub fn approve_shader(fingerprint: &str) {
    if !note_approved(fingerprint) {
        return;
    }
    let fingerprint = fingerprint.to_string();
    Settings::update(move |s| {
        s.session.approved_shaders.insert(fingerprint);
    });
}

/// Drop a hash from the live list. Nothing in the UI revokes one yet; this
/// is what the gate's tests, here and up in rox, clean up after themselves
/// with.
pub fn forget_approved(fingerprint: &str) {
    APPROVED_SHADERS.write().unwrap().remove(fingerprint);
}

/// The live shader pool, cached out of the look the app is wearing. Read
/// where a shader is about to register, which is a render path with no
/// business touching the disk, so the file is read once on the first look
/// and every write goes through [`set_shader_pool`], which keeps the cache
/// and the file in step. The same shape as [`APPROVED_SHADERS`] above, for
/// the same reason.
static SHADER_POOL: LazyLock<RwLock<Vec<NamedShader>>> =
    LazyLock::new(|| RwLock::new(Settings::load().look.bundle.shaders));

/// How many times the pool has been replaced. A surface resolves its name
/// once and holds the answer; this is what tells it the answer went stale
/// without diffing a few kilobytes of WGSL every frame.
static SHADER_POOL_REV: AtomicU64 = AtomicU64::new(0);

/// Everything in the pool. Cloned out rather than handed a guard: entries
/// are a name and a page of text, and holding the lock across a render would
/// mean a shader edit blocking paint.
pub fn shader_pool() -> Vec<NamedShader> {
    SHADER_POOL.read().unwrap().clone()
}

/// One pool entry by name, or None when the look doesn't carry it.
pub fn shader_pool_get(name: &str) -> Option<NamedShader> {
    SHADER_POOL
        .read()
        .unwrap()
        .iter()
        .find(|shader| shader.name == name)
        .cloned()
}

/// Replace the pool, here and on disk. The pool belongs to the look, so it
/// persists into the bundle the app is wearing and travels with the next
/// export.
pub fn set_shader_pool(shaders: Vec<NamedShader>) {
    note_shader_pool(shaders.clone());
    Settings::update(move |s| {
        s.look.bundle.shaders = shaders;
    });
}

/// Replace the pool in the cache alone. The half of a pool write that costs
/// nothing, split out the way [`note_approved`] is: the tests use it to
/// exercise resolution without a settings file underneath them, and a
/// workspace apply uses it because it has already written the whole bundle
/// in one go and a second write would only rewrite the same field.
pub fn note_shader_pool(shaders: Vec<NamedShader>) {
    *SHADER_POOL.write().unwrap() = shaders;
    SHADER_POOL_REV.fetch_add(1, Ordering::Relaxed);
}

/// The pool's generation. Bumped on every replacement, so a cached
/// resolution can be checked with one atomic load instead of a comparison
/// against the source it came from.
pub fn shader_pool_rev() -> u64 {
    SHADER_POOL_REV.load(Ordering::Relaxed)
}

/// The live look's backdrop shader config, cached out of the settings file
/// the way the pool is: the workspace root reads it every render, which is
/// no place for disk.
static BACKDROP_SHADER: LazyLock<RwLock<Option<PostShaderConfig>>> =
    LazyLock::new(|| RwLock::new(Settings::load().look.bundle.backdrop_shader.clone()));

/// What the backdrop wears, or None for a bare art wash.
pub fn backdrop_shader() -> Option<PostShaderConfig> {
    BACKDROP_SHADER.read().unwrap().clone()
}

/// Replace the backdrop shader in the cache alone, [`note_shader_pool`]'s
/// twin: a workspace apply has already written the whole bundle.
pub fn note_backdrop_shader(config: Option<PostShaderConfig>) {
    *BACKDROP_SHADER.write().unwrap() = config;
}

/// One connected account. Last.fm binds a session to the api key it was
/// authorized under, so this is only ever usable by a build signing with
/// that same key.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LastfmSession {
    /// What the connect flow landed. Never expires until it's revoked on
    /// Last.fm. Empty means this api key has no session: it never
    /// connected, the user disconnected, or Last.fm refused what it had.
    pub key: String,
    /// The account it belongs to, for the settings readout.
    pub username: String,
}

impl LastfmSession {
    fn connected(&self) -> bool {
        !self.key.is_empty()
    }
}

/// The Last.fm account and how scrobbling behaves. The key and secret
/// override the build's own api identity (`lastfm::keys`), for builds
/// that ship none.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Lastfm {
    pub api_key: String,
    pub api_secret: String,
    /// The sessions this machine holds, filed under the api key that
    /// minted each. A map rather than one session because the identity
    /// varies by where the build came from: the nix package, the release
    /// workflow, and a local `.env` build each sign with their own pair,
    /// and they all read this one file. One session per key means moving
    /// between them costs a connect the first time and nothing after.
    ///
    /// The empty key is the unattributed slot, holding the single session
    /// a file from before this split carried. Whichever build it belongs
    /// to claims it on the first call that lands ([`Self::attribute`]);
    /// every other build files its own refusal and stops reaching for it.
    pub sessions: BTreeMap<String, LastfmSession>,
    /// The session a pre-`sessions` file carried, with nothing recording
    /// which api key minted it. Read once on load into the unattributed
    /// slot, never written back.
    #[serde(skip_serializing)]
    session_key: String,
    #[serde(skip_serializing)]
    username: String,
    /// Whether playback scrobbles at all; the connection stays either way.
    pub scrobbling: bool,
    /// Whether the heart mirrors out as a Last.fm love. Off by default,
    /// unlike scrobbling: connecting an account is consent to publish what
    /// played, not to rewrite the loved list a user may have curated over
    /// there for years. Turning it on mirrors from that point forward, it
    /// never pushes the favourites already on the shelf.
    pub love_favourites: bool,
    /// How much of a track has to actually play before it scrobbles, as a
    /// fraction of its duration. The seek strip and waveform can mark it.
    pub threshold: f32,
}

impl Default for Lastfm {
    fn default() -> Self {
        Lastfm {
            api_key: String::new(),
            api_secret: String::new(),
            sessions: BTreeMap::new(),
            session_key: String::new(),
            username: String::new(),
            scrobbling: true,
            love_favourites: false,
            threshold: 0.5,
        }
    }
}

/// The slot holding a session no api key has claimed yet.
const UNATTRIBUTED: &str = "";

impl Lastfm {
    /// The session a build signing with `api_key` can actually use: its
    /// own, or the unattributed one while this key has never tried. A
    /// build with no identity at all gets None, since nothing it sent
    /// could be signed anyway.
    ///
    /// An entry that's present but empty is a key that asked and was
    /// refused, which is the whole reason it's stored: without it, every
    /// launch would reach for a session it has already been told isn't
    /// its own.
    pub fn session(&self, api_key: &str) -> Option<&LastfmSession> {
        if api_key.is_empty() {
            return None;
        }
        match self.sessions.get(api_key) {
            Some(session) => session.connected().then_some(session),
            None => self.sessions.get(UNATTRIBUTED).filter(|s| s.connected()),
        }
    }

    /// The account name for the settings readout, empty where this build
    /// holds no session.
    pub fn username(&self, api_key: &str) -> &str {
        self.session(api_key).map_or("", |s| s.username.as_str())
    }

    /// Whether some other api key holds a session, for telling "never
    /// connected" apart from "connected, but under a different build".
    pub fn connected_elsewhere(&self, api_key: &str) -> bool {
        self.sessions
            .iter()
            .any(|(key, session)| key != api_key && session.connected())
    }

    /// File the session the connect flow just landed under the key that
    /// minted it.
    pub fn connect(&mut self, api_key: &str, key: String, username: String) {
        self.sessions
            .insert(api_key.to_string(), LastfmSession { key, username });
    }

    /// Leave this build without a session: what Disconnect does, and
    /// where a refusal from Last.fm lands. The entry stays behind empty
    /// rather than going away, because an absent key is one that hasn't
    /// tried the unattributed session yet and this one has.
    ///
    /// A build with no identity has nothing to clear, and writing its
    /// refusal would take the unattributed slot and the session sitting
    /// in it down with it.
    pub fn clear_session(&mut self, api_key: &str) {
        if api_key.is_empty() {
            return;
        }
        self.sessions
            .insert(api_key.to_string(), LastfmSession::default());
    }

    /// Claim the unattributed session for the key that just used it
    /// successfully, which is the only proof of who minted it there is.
    /// True when that moved something, so the caller knows to persist.
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

    /// Fold a pre-`sessions` file's flat session into the unattributed
    /// slot. Nothing on disk says which build authorized it, so it goes
    /// in unclaimed and the first call that lands names it.
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

/// Where a fetched lyrics sheet saves: the embedded tag through the
/// writer's atomic layer, an `.lrc` sidecar next to the audio file, or
/// the app's own lyrics store under [`lyrics_dir`].
#[derive(Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LyricsSave {
    Tag,
    Sidecar,
    /// The default: rox's data folder, so fetches never leave files in
    /// the library and never rewrite the audio.
    #[default]
    Store,
}

/// The app's own lyrics store inside [`data_dir`], one flat folder of
/// hashed-name `.lrc` files. Not created here: the first save makes it,
/// so an unused store never leaves an empty folder behind.
pub fn lyrics_dir() -> PathBuf {
    data_dir().join("lyrics")
}

/// The artist store inside [`data_dir`]: the biography panel's fetched
/// bios and portraits, one hashed-name pair per artist. Not created
/// here: the first fetch makes it, the lyrics store's rule.
pub fn artists_dir() -> PathBuf {
    data_dir().join("artists")
}

/// The online enrichment providers (ADR 14): per-service enable toggles
/// and the per-domain knobs. Providers only ever fetch on a user action,
/// so on-by-default keeps the offline-first rule intact.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Providers {
    /// Fetch lyrics from lrclib.net when the lyrics panel asks.
    pub lrclib: bool,
    /// Where a fetched sheet lands.
    #[serde(deserialize_with = "lenient::or_default")]
    pub lyrics_save: LyricsSave,
    /// Look up tags on MusicBrainz when the metadata compare asks.
    pub musicbrainz: bool,
    /// Search iTunes for cover art when the cover lookup asks.
    pub itunes: bool,
    /// Search Deezer for cover art when the cover lookup asks.
    pub deezer: bool,
    /// Search Last.fm for cover art when the cover lookup asks.
    pub lastfm_art: bool,
    /// Fetch artist biographies from Last.fm, a Deezer portrait along,
    /// when the biography panel asks.
    pub artist: bool,
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
        }
    }
}

/// The graphic equalizer's saved curve (ADR 19): whether it shapes the
/// output at all, and the per-band gains in dB in
/// [`rox_playback::eq::BAND_HZ`] order. The live values are atomics the
/// settings page writes straight into (see rox's `player::set_eq_gain`);
/// this is only what they're seeded from and flushed back to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqSettings {
    pub enabled: bool,
    /// A list rather than a fixed array so a file written against a
    /// different band count still loads: extra values are dropped and
    /// missing ones read flat.
    pub gains: Vec<f32>,
    /// Where each band sits, in Hz. Empty in a file written before the
    /// centers could move, which loads onto the ISO octaves the graphic EQ
    /// had them welded to.
    #[serde(default)]
    pub freqs: Vec<f32>,
    /// How wide each band is. Empty loads at one octave, the old fixed
    /// width.
    #[serde(default)]
    pub qs: Vec<f32>,
    /// How the live analyzer behind the curve is drawn, if at all.
    #[serde(default, deserialize_with = "lenient::or_default")]
    pub analyzer: AnalyzerStyle,
    /// The analyzer's window, in samples. Snapped to a power of two the
    /// analyzer takes when it's read, so a hand-edited number can't panic
    /// the window it opens.
    pub fft_size: usize,
}

/// How the equalizer draws the music behind its curve. The analyzer is
/// context for the shaping, never the subject, so the default is the shape
/// that stays out of the way: bars carry more detail but read as the loudest
/// thing on screen, and the curve is what's being edited.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnalyzerStyle {
    /// One smoothed outline, filled underneath.
    #[default]
    Wave,
    /// A bar per band, the spectrum panel's shape.
    Bars,
    /// Nothing behind the curve at all.
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
            // Long, because what the analyzer is here for is showing where a
            // band sits: at a short window the bottom two octaves land in a
            // handful of bins and the bass reads as one smear.
            fft_size: 8192,
        }
    }
}

/// How tagged loudness is levelled (ADR 19). Off by default: leveling is
/// processing, and a player that quietly turns every track down without
/// being asked is one you can't trust the bit-perfect claim from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplayGainSettings {
    /// Which of a file's two gains to read, or none at all.
    #[serde(deserialize_with = "lenient::or_default")]
    pub mode: GainModeSetting,
    /// Added to every tagged gain, in dB. ReplayGain's reference sits well
    /// below where modern masters are cut, so a levelled library plays
    /// quieter than the same library raw; this is where that's taken back.
    pub preamp_db: f32,
    /// What a file with no ReplayGain tags plays at, in dB. Its own knob
    /// rather than the preamp: an untagged track has nothing to be offset
    /// from, so the number is the whole decision.
    pub fallback_db: f32,
    /// Where the measurement pass puts what it measured. Nothing the engine
    /// reads: it sits here because it's about levelling, and the job reads
    /// it once when it starts.
    #[serde(deserialize_with = "lenient::or_default")]
    pub save: ReplayGainSave,
    /// Whether the measurement pass follows the watcher, so files that land
    /// in the library while rox is running get measured without anyone asking
    /// (ADR 19). Off by default: measuring decodes every file, and in tags
    /// mode it rewrites them, neither of which should start on its own until
    /// it's been agreed to once.
    pub auto: bool,
}

/// Where a measured ReplayGain lands, the Audio page's pick (ADR 19).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplayGainSave {
    /// The default: rox's own library database, so a measurement pass never
    /// rewrites a file and never bumps an mtime.
    #[default]
    Database,
    /// The file's own tags, through the writer's atomic layer, so every
    /// other player reads the same numbers. Rewrites the audio files.
    Tags,
}

/// Where an acoustic vector lands, the Library page's pick.
///
/// [`ReplayGainSave`]'s shape with one difference that matters: the database
/// row is written either way. A vector is only useful through the similarity
/// query, and that query reads the table, so tags mode is a second copy
/// rather than a different destination. What it buys is a description that
/// outlives the database: wipe the library, or carry the folder to another
/// machine, and the files still say what they sound like.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AcousticSave {
    /// The default: the library database alone, so a pass never rewrites a
    /// file and never bumps an mtime.
    #[default]
    Database,
    /// The database and the file's own tags. MP3 and FLAC only, since those
    /// are the formats the writer handles; every other format keeps its
    /// database row and nothing else.
    Tags,
}

/// The persisted spelling of [`rox_playback::gain::GainMode`], kept apart
/// from it so the settings file stays readable words rather than whatever
/// the engine's enum happens to derive.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GainModeSetting {
    #[default]
    Off,
    Track,
    Album,
}

impl ReplayGainSettings {
    /// The rule the engine levels by.
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

/// How samples reach the device (ADR 19): which backend opens the stream
/// and which device it claims. These are a request. What the hardware
/// agreed to is on the running session, and the Audio page shows that
/// rather than these values.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputSettings {
    /// Whether output claims the device for rox alone, at the file's own
    /// rate where the hardware takes one. Off shares the system mixer with
    /// every other app, which is where rox has always been. A claim that
    /// fails falls back to shared with the reason shown, never to silence.
    pub exclusive: bool,
    /// The shared-mode device, by the name cpal calls it. None follows the
    /// system default, and so does a name that isn't on this machine.
    pub device: Option<String>,
    /// The exclusive-mode device, by its ALSA name. Kept apart from the
    /// pick above because the two id spaces don't cross: a cpal device name
    /// means nothing to ALSA, and vice versa.
    pub exclusive_device: Option<String>,
    /// The rate exclusive mode runs at, or None to follow each file's own
    /// (ADR 19), which is the setting that makes a mixed-rate library play
    /// bit-perfect throughout. Pinning trades that for never paying the
    /// reopen gap at a boundary, worth it on a card whose clock hates
    /// switching.
    #[serde(default)]
    pub rate: Option<u32>,
    /// The sample format exclusive mode asks for by short name (`f32`,
    /// `s32`, `s16`), or None for the widest the device offers. A card that
    /// won't take the pick runs the widest anyway and says so.
    #[serde(default)]
    pub format: Option<String>,
    /// The exclusive device's period in milliseconds, or None for the
    /// backend's 10 ms. Lower wakes the writer thread more often, which is
    /// what starts crackling on a loaded machine.
    #[serde(default)]
    pub period_ms: Option<f64>,
}

/// Discord Rich Presence settings: enable toggle and metadata options.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscordSettings {
    /// Whether Discord Rich Presence is enabled.
    pub enabled: bool,
    /// Whether "View on Last.fm" button is shown.
    pub show_lastfm_button: bool,
    /// Whether "Search on YouTube" button is shown.
    pub show_youtube_button: bool,
}

impl Default for DiscordSettings {
    fn default() -> Self {
        DiscordSettings {
            enabled: false,
            show_lastfm_button: true,
            show_youtube_button: true,
        }
    }
}

/// A dock layout the user saved as a named preset: a full dock dump under
/// a name. The dump stays raw JSON like [`Settings::layout`] so the file
/// survives layout-schema moves; the workspace validates it on apply.
#[derive(Clone, Serialize, Deserialize)]
pub struct NamedLayout {
    pub name: String,
    pub dump: serde_json::Value,
    /// The window size the preset restores to, in logical pixels. None for
    /// presets from before sizes were stored, which apply at whatever size
    /// the window already has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<LayoutSize>,
}

/// A single configured panel the user saved under a name: the panel's own
/// dump, the leaf a layout carries per panel. Adding one back builds the
/// panel with its config, its rename, and whatever children a composite
/// holds, so a dialed-in panel is reproducible without redoing its settings.
///
/// The dump stays raw JSON for the reasons [`NamedLayout`]'s does: rox-core
/// stays off the dock crate, and the file survives a config-schema move.
#[derive(Clone, Serialize, Deserialize)]
pub struct PanelPreset {
    pub name: String,
    /// The dock's `PanelState` as JSON: the panel's registry name, its config
    /// blob, and its children.
    pub panel: serde_json::Value,
}

impl PanelPreset {
    /// The registry name of the panel inside, so a menu can find its icon and
    /// placement without deserializing the dump. None for a blob that isn't
    /// shaped like a panel state.
    pub fn panel_name(&self) -> Option<&str> {
        self.panel.get("panel_name")?.as_str()
    }
}

/// A window size in logical pixels, stored with a layout preset so applying
/// it can size the window to match.
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutSize {
    pub width: f32,
    pub height: f32,
}

/// A layout's unsaved working state: the dock dump plus the window size it
/// was last at, kept in [`Settings::layout_edits`] so switching back restores
/// both the arrangement and the size without touching the saved preset.
#[derive(Clone, Serialize, Deserialize)]
pub struct LayoutEdit {
    pub dump: serde_json::Value,
    /// The window size when the edit was stashed. None for a copy from before
    /// sizes rode along, which falls back to the preset's saved size on apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<LayoutSize>,
}

/// The workspace bundle format version, bumped when the bundle shape changes
/// so a reader can refuse a file from a newer format. Independent of the dock
/// layout version the dumps inside carry.
pub const WORKSPACE_VERSION: u32 = 1;

/// A shareable workspace: a named set of layout presets with their
/// mini-player roles, the palette, and the appearance that dress them. The
/// unit rox's sharing ecosystem trades - written to a file by export, shipped
/// in the app's assets, and imported into the collection. Versioned so a file
/// survives shape moves; the layouts inside carry their own dock-layout
/// version the workspace validates on apply. Machine- and account-bound state
/// (library folders, Last.fm, window frames) is deliberately left out, so a
/// bundle travels between installs as pure look.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceBundle {
    /// Format version; a reader refuses a bundle from a newer format.
    pub version: u32,
    /// The bundle's name. A shipped bundle falls back to its file stem when
    /// this is empty, the layouts' own convention.
    pub name: String,
    /// The layout presets the workspace carries, each a named dock dump.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient::vec"
    )]
    pub layouts: Vec<NamedLayout>,
    /// The panel presets the workspace carries, each a named single panel.
    /// They ride the bundle rather than the user's settings because a panel
    /// can name a shader out of the pool below, and that name only means
    /// something while this workspace's pool is the live one.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient::vec"
    )]
    pub panel_presets: Vec<PanelPreset>,
    /// The mini-player button's two roles, by preset name, scoped to this
    /// workspace's own layouts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_layout: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mini_layout: Option<String>,
    /// The two theme palettes as role-name-to-`#rrggbb`,
    /// [`Palette::to_map`]'s shape; an empty map means that theme's
    /// designed defaults.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub palette_dark: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub palette_light: BTreeMap<String, String>,
    /// The shared signal pool the workspace's looks ride: a layout that
    /// pulses to the kick is meaningless without "Kick", so the pool
    /// travels with the look and an apply replaces it wholesale.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient::vec"
    )]
    pub signals: Vec<Signal>,
    /// The shader pool the workspace's looks point into: every named WGSL
    /// the bundle carries, in one place. A panel that names "Grain" is
    /// meaningless without it, so the pool travels with the look exactly the
    /// way the signal pool above does, and an apply replaces it wholesale.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "lenient::vec"
    )]
    pub shaders: Vec<NamedShader>,
    /// Who made this workspace and what it is. Empty on a look nobody has
    /// filled in, which is every look until it's exported with a card.
    #[serde(skip_serializing_if = "WorkspaceMeta::is_empty")]
    pub meta: WorkspaceMeta,
    /// The whole-window shader the workspace wears, the screen-sized twin of
    /// the per-panel ones its layouts carry.
    ///
    /// None applies as the disabled default rather than as "leave what's
    /// there". An apply replaces the look wholesale, and a workspace that
    /// says nothing about a screen shader means a look without one, so
    /// switching to it can't leave the last look's shader running over it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_shader: Option<PostShaderConfig>,
    /// The shader painted over the backdrop and under everything else, so
    /// it only ever reads the art wash and the panels stay untouched. The
    /// same config shape as the screen shader, but it lives in the bundle
    /// rather than the machine settings: a backdrop treatment is part of
    /// the look, not of this install. None means a bare backdrop, the
    /// same replace-wholesale read as `post_shader`'s.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backdrop_shader: Option<PostShaderConfig>,
    /// The appearance knobs the workspace dresses the app with.
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

/// The card on a workspace: who made it, what it is, and where it came from.
/// Every field is free text and empty means unset, because this is the half
/// of a bundle nothing reads but a person. It exists so a shared workspace
/// arrives with an author's name on it instead of a filename.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceMeta {
    /// Who made it.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub author: String,
    /// A line or two on what the look is going for.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Where it lives: the author's page, a repo, a forum thread.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub website: String,
    /// The author's own version string, whatever they count in. Nothing to
    /// do with [`WORKSPACE_VERSION`], which is the file format's.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// The terms it's shared under, if the author says.
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
    /// Whether anybody has filled in anything at all, which is what keeps an
    /// empty card out of the file.
    pub fn is_empty(&self) -> bool {
        self.author.is_empty()
            && self.description.is_empty()
            && self.website.is_empty()
            && self.version.is_empty()
            && self.license.is_empty()
            && self.created.is_empty()
            && self.updated.is_empty()
    }

    /// Date the card for an export. `updated` always moves; `created` is
    /// written once and then left alone, since it's the day the workspace
    /// first existed and every export after that would only overwrite it
    /// with today.
    pub fn stamp(&mut self, today: &str) {
        if self.created.is_empty() {
            self.created = today.to_string();
        }
        self.updated = today.to_string();
    }

    /// Take what the card being replaced said wherever this one says
    /// nothing. Saving over a workspace is a fresh snapshot of the same
    /// look, so the card somebody filled in belongs to it just as much as
    /// the layouts do; wiping it because the live look never carried one
    /// would throw away work nobody asked to lose.
    ///
    /// `created` always comes back, since it's the day the workspace first
    /// existed and the replacement has no way to know it. `updated` is left
    /// alone, so whatever stamped this card keeps today's date. Everything
    /// else only fills a gap, which is what lets a live look that carries
    /// its own author keep it.
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

/// Today's date in UTC as `YYYY-MM-DD`, the stamp on an exported bundle.
/// UTC rather than local because the date on a shared file shouldn't depend
/// on which side of midnight the exporter's timezone happens to be.
fn utc_today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// The appearance a workspace carries: the visual knobs it dresses the app
/// with, pulled from and pushed back to [`Settings`]. The subset that reads
/// as pure look, so a bundle recolors and rearranges without dragging along
/// another machine's folders or account. The theme pick stays out: a
/// workspace brings both palettes and the user's dark/light/System choice
/// decides which one shows. The app font size stays out for the same reason -
/// it's a per-user readability choice, not a look to hand around, so applying
/// a workspace never resizes the text out from under someone.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceBundle {
    /// ADR 10's transparency pair, both 0 to 1. How opaque the app's
    /// surfaces read, 1 fully opaque...
    pub surface_opacity: f32,
    /// ...and how strongly the backdrop shows behind them, 1 the bare
    /// bake, 0 sunk into the floor.
    pub backdrop_strength: f32,
    /// Whether the child windows (settings, editors, dialogs, popped-out
    /// panels) paint the cover backdrop too, wearing the transparency the
    /// same way the workspaces do. On by default; off keeps the treatment
    /// to the workspace windows and the children on their plain surfaces.
    pub backdrop_all_windows: bool,
    /// The app-wide frame defaults every panel inherits: margin, padding,
    /// rounding, and border, all in px. A panel's own theme overrides any
    /// of them; unset there, the panel takes these.
    pub frame: Frame,
    /// Whether the 1px seams between panel tiles paint. Off leaves the
    /// resize grips invisible but still draggable, so panels sit flush.
    pub seams: bool,
    /// Whether the playing track's art re-tints the palette and backs
    /// the windows (ADR 10's derived mode). Off by default: the look
    /// only follows the music when asked to.
    pub art_theming: bool,
    /// Whether song theming is held to the active theme. Song theming
    /// still tints hue and chroma, but a cover's brightness never swaps
    /// the light and dark palettes. Off by default: the app follows a
    /// bright album all the way.
    pub keep_theme: bool,
    /// The app-wide font family, the base every window and panel inherits.
    /// None follows the platform default. A panel's own font override layers
    /// over this; a name that is not installed falls back at render, so the
    /// file survives moving between machines.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_font: Option<String>,
    /// How ratings read and click everywhere they show.
    #[serde(deserialize_with = "lenient::or_default")]
    pub rating_style: RatingStyle,
    /// Whether unfilled star slots draw a faint dot, so an unrated row
    /// reads as a quiet row of dots instead of empty space.
    pub rating_dots: bool,
    /// The quick-play modal's appearance knobs, edited from its own config
    /// panel.
    pub quick_play: QuickPlayConfig,
    /// Whether the in-window menubar stays hidden, showing only while alt
    /// is held or a menu is open. Off by default: the bar is the way into
    /// everything.
    pub hide_menubar: bool,
    /// Whether the main workspace windows carry the OS's own decorations
    /// (titlebar, borders). Off asks the compositor for a bare
    /// client-drawn window; the window controls panel stands in for the
    /// missing buttons. Child windows (settings, popouts, editors) keep
    /// the OS chrome either way.
    pub os_decorations: bool,
    /// Whether the main windows resize by dragging their edges. Windows
    /// only, and only once the OS decorations are off: with them on the OS
    /// owns the frame and its border. Off keeps the frame itself, so the
    /// shadow, snap layouts and Win+arrow still work, and only the resize
    /// cursor at the edges goes away.
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
            hide_menubar: false,
            os_decorations: true,
            resize_border: true,
        }
    }
}

/// The look the app is wearing: `workspace.json`'s whole contents. The bundle
/// half is the shareable look, the same shape a saved workspace file holds, so
/// saving one out is a copy rather than a field-by-field transcription. The
/// working state under it never travels: it's about this dock on this machine,
/// not about how the app looks.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LookState {
    /// The live look. Its `name` records the workspace it was applied from,
    /// so the UI can say which one you're on; empty for a look that was never
    /// applied from a saved workspace.
    pub bundle: WorkspaceBundle,
    /// The dock layout as the dock crate's own serialized state, kept as raw
    /// JSON so the file stays readable even when the layout schema moves; the
    /// workspace validates and versions it on restore. None until a layout
    /// has been saved.
    pub layout: Option<serde_json::Value>,
    /// The named preset the window is currently on, by name, so a workspace
    /// save captures the layout in front of you and the mini button knows
    /// which side it is on. None means an unnamed arrangement (the default
    /// build, an empty window, or a one-off import).
    pub active_layout: Option<String>,
    /// Per-layout working copies: the unsaved dock tweaks for each named
    /// layout that is not the one in front of you, keyed by layout name.
    /// Switching layouts stashes the outgoing one here and restores the
    /// incoming one's copy, so edits survive a switch and a relaunch without
    /// touching the saved preset. The layout in front of you keeps its live
    /// dock in `layout` instead; an explicit save folds a copy into its
    /// preset and clears it here.
    #[serde(
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "lenient::map"
    )]
    pub layout_edits: BTreeMap<String, LayoutEdit>,
}

impl LookState {
    /// Rebuild the look from a pre-split `settings.json`, where every look
    /// field sat flat beside the machine state. The bundle's own fields kept
    /// their names through the move, and so did the appearance knobs, so both
    /// halves deserialize straight out of the old flat map without a field
    /// list to keep in sync. Runs while `workspace.json` is missing; the next
    /// save writes the split files and the stale keys drop as unknown fields.
    fn from_legacy(value: &serde_json::Value) -> LookState {
        let mut bundle: WorkspaceBundle = serde_json::from_value(value.clone()).unwrap_or_default();
        // The appearance knobs were top-level siblings before the split, not
        // a nested object, so they need their own pass over the same map.
        bundle.appearance = serde_json::from_value(value.clone()).unwrap_or_default();
        // A pre-split file records no workspace name: the look is whatever it
        // has been edited into, not a named one.
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

/// The Shader panel's dock name, the one panel whose own config carries a
/// source and a file bookmark instead of wearing one as chrome. Spelled here
/// because the scrub walks dumps as raw JSON, well below the crate that
/// defines the panel.
const SHADER_PANEL: &str = "shader";

/// Walk a dock dump and take the shader file bookmarks out of it. Recursive
/// because a dump is a tree of dock nodes and a panel can be at any depth.
fn scrub_dump_paths(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            // Any panel's surface shader, flattened onto its config.
            if let Some(serde_json::Value::Object(shader)) = map.get_mut("shader") {
                shader.remove("path");
            }
            // The Shader panel, whose config is the shader.
            if map.get("panel_name").and_then(|name| name.as_str()) == Some(SHADER_PANEL) {
                if let Some(serde_json::Value::Object(config)) =
                    map.get_mut("info").and_then(|info| info.get_mut("panel"))
                {
                    config.remove("path");
                }
            }
            for child in map.values_mut() {
                scrub_dump_paths(child);
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(scrub_dump_paths),
        _ => {}
    }
}

/// Every shader source a dock dump carries, in whatever order the walk finds
/// them. One of a family of walks over the same two shapes ([`scrub_dump_paths`],
/// [`dump_wears_shader`], [`strip_dump_shaders`]), kept apart because some take
/// the tree by `&mut` and some can't, and Rust has no way to write one walk over
/// both. Whatever gets added to one belongs in all of them.
///
/// This is what the startup trust pass hands [`trust_shipped`], so a shipped
/// look's panels paint without asking anyone to agree to code that came with
/// the binary.
pub fn dump_shader_sources(value: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    collect_dump_shader_sources(value, &mut out);
    out
}

/// Whether anything in a dock dump would actually paint a shader: a panel
/// wearing one as chrome, or the Shader panel itself. The question the apply
/// confirm asks to decide whether the look gets the with-shaders choice at
/// all, which is about what runs rather than about what the machine trusts.
///
/// A pool name counts the same as inline text. A name that resolves to
/// nothing paints nothing, but that's a question for the pool the apply is
/// about to install, not for a config that has said what it wants.
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
            // The Shader panel, which is a shader by definition: it counts
            // unless its config has been emptied out or switched off. A
            // config saying nothing at all runs the shipped example.
            if map.get("panel_name").and_then(|name| name.as_str()) == Some(SHADER_PANEL) {
                let config = map.get("info").and_then(|info| info.get("panel"));
                let quiet = match config {
                    Some(serde_json::Value::Object(config)) => {
                        // Source text that's there and blank is a panel
                        // somebody emptied; a config with no source line at
                        // all is one that never said, and runs the default.
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

/// Whether a shader config's field holds a string with something in it.
fn has_text(map: &serde_json::Map<String, serde_json::Value>, key: &str) -> bool {
    map.get(key)
        .and_then(|v| v.as_str())
        .is_some_and(|text| !text.trim().is_empty())
}

/// Walk a dock dump and switch every shader in it off: the chrome one panels
/// wear, and the Shader panel's own. The write twin of
/// [`dump_wears_shader`], for a workspace applied without the shaders it
/// brought.
///
/// Off, not gone. The source, the pool name and the routes all stay on the
/// config, so a look lands quiet and every shader it brought is one toggle
/// away on the panel that wears it. Nothing runs on the way in: an unread
/// source still has to get past the approval block, and a switch that's down
/// paints nothing whatever the trust says.
///
/// Deleting them was the old reading of the button, and it left the Shader
/// panel with an empty config and no way back to the shader the look came
/// with.
pub fn strip_dump_shaders(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Object(shader)) = map.get_mut("shader") {
                shader.insert("enabled".into(), serde_json::Value::Bool(false));
            }
            if map.get("panel_name").and_then(|name| name.as_str()) == Some(SHADER_PANEL) {
                // A panel that saved no config of its own still runs the
                // shipped example, so the switch has to be written even
                // where there's nothing else to write it beside.
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
            if let Some(serde_json::Value::Object(shader)) = map.get("shader") {
                if let Some(source) = shader.get("source").and_then(|s| s.as_str()) {
                    out.push(source.to_string());
                }
            }
            // The Shader panel, whose config is the shader.
            if map.get("panel_name").and_then(|name| name.as_str()) == Some(SHADER_PANEL) {
                if let Some(serde_json::Value::Object(config)) =
                    map.get("info").and_then(|info| info.get("panel"))
                {
                    if let Some(source) = config.get("source").and_then(|s| s.as_str()) {
                        out.push(source.to_string());
                    }
                }
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
    /// Snapshot the current shareable state into a named bundle: the layouts
    /// and their roles, the palette, and the appearance. Reads the persisted
    /// settings, which every live edit already writes through.
    ///
    /// Folds the live dock into the layout the window is on, but only inside
    /// this bundle's own copy of the layouts: a save captures what is in front
    /// of you without editing the global preset pool other workspaces share,
    /// since layouts belong to the workspace they were saved in, not to a
    /// shared pool. An unnamed arrangement lands as "Untitled", and when the
    /// bundle has no primary the captured layout becomes it, so it fills the
    /// window on apply. No live dock yet leaves the layouts as they are.
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
        // The screen shader sits in the machine settings rather than in the
        // look, since what it reads is a local path, so it gets copied in by
        // hand here. It rides in before the two passes below, which are what
        // turn that local path into something that travels.
        if s.post_shader.configured() {
            bundle.post_shader = Some(s.post_shader.clone());
        }
        bundle.inline_post_shader();
        bundle.scrub_paths();
        bundle.meta.stamp(&utc_today());
        bundle
    }

    /// Pull the screen shader's file into the bundle, so it travels. A
    /// pass configured the old way points at a path and nothing else, and a
    /// path is the one thing that means nothing on the machine this lands
    /// on. Best effort: a file that's gone or unreadable leaves the source
    /// empty, which is the same dead pass the bundle would have carried
    /// anyway, and there's nobody to tell at export time.
    pub fn inline_post_shader(&mut self) {
        for shader in [self.post_shader.as_mut(), self.backdrop_shader.as_mut()]
            .into_iter()
            .flatten()
        {
            if !shader.source.is_empty() {
                continue;
            }
            if let Some(path) = shader.path.as_ref() {
                if let Ok(source) = std::fs::read_to_string(path) {
                    shader.source = source;
                }
            }
        }
    }

    /// Drop every local file bookmark on the way out. Paths are the one part
    /// of a shader that can't travel: at best they point at nothing on the
    /// machine that imports the bundle, and at worst they aim a hot reload
    /// at a file that happens to exist there and belongs to somebody else.
    /// The sources came along inline, so nothing is lost.
    ///
    /// The dumps get walked rather than reserialized, since this layer has
    /// no idea what a panel config looks like. Two shapes carry a bookmark:
    /// any panel's surface shader, which rides its config flattened under
    /// `shader`, and the Shader panel's own config, which keeps its source
    /// and path at the top level of the dock node's panel info. Both are
    /// targeted by name rather than by stripping every `path` key in sight,
    /// which would take a folder panel's root with it.
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

    /// Replace the settings' shareable state with this bundle's, the apply's
    /// persistence half. The live dock and the preset it sits under stay put:
    /// the layout swap belongs to the caller, which has the workspace whose
    /// dock it changes. The live statics stay the caller's too, since they
    /// need an `App` this layer doesn't reach.
    pub fn apply_to(self, s: &mut Settings) {
        // A workspace brings its own presets; drop any working copies keyed to
        // the old look so they can't shadow the incoming layouts.
        s.look.layout_edits.clear();
        s.look.bundle = self;
    }
}

/// The closing snapshot of the playing track: its library id and the
/// position clock in seconds. Superseded by [`QueueState`] for files written
/// since; kept as the single-track fallback so an older settings file still
/// restores something.
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LastTrack {
    pub id: i64,
    /// Which subsong of its file the track was: 0 for a plain file, the cue
    /// sheet's track number for a span of an image. Defaulted, so a file
    /// written before cue support reads as a plain file, which is what every
    /// track in it was.
    pub sub: u16,
    pub position_secs: f64,
}

/// The closing snapshot of the whole play queue, restored as a full session
/// on the next launch so Prev and Next walk the same order and the up-next
/// queue panel comes back. Entries are library ids so they survive path
/// changes; one whose file has left the library drops out on restore, the
/// cursor shifting to stay on the track that was playing.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct QueueState {
    /// The play order, history and upcoming both, in the order the engine
    /// held them.
    pub entries: Vec<QueuedTrack>,
    /// Index into `entries` of the track that was playing.
    pub cursor: usize,
    /// Where that track's clock sat, in seconds.
    pub position_secs: f64,
}

/// One entry in a persisted [`QueueState`]: the track's library id and
/// whether it was hand-queued (Play Next, Add to Queue) rather than part of
/// the playing context. The queue panel lists only the explicit ones.
#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct QueuedTrack {
    pub id: i64,
    /// Which subsong of its file the entry is, 0 for a plain file. Carried
    /// beside the id rather than left to be re-derived: the restore resolves
    /// the id to a path before the projection is necessarily up, and without
    /// this a whole-disc rip would come back as twelve copies of the image.
    /// Defaulted per field, since the struct itself isn't, so a session file
    /// written before cue support still reads instead of costing the queue.
    #[serde(default)]
    pub sub: u16,
    pub explicit: bool,
}

/// The tag editor's remembered shape: window size in logical pixels,
/// the table's column widths (one slot per column in field order, shown
/// or not), the columns hidden from the table, and the last guess
/// pattern. Every editor window writes it on close, the last writer
/// wins.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TagEditorState {
    pub width: f32,
    pub height: f32,
    pub columns: Vec<f32>,
    pub hidden: Vec<String>,
    pub pattern: String,
}

/// The rename dialog's remembered shape: window size in logical pixels
/// and the patterns that were last applied, newest first. The list is
/// the point of remembering: one library tends to a couple of naming
/// schemes, and retyping the good one every time is the whole friction.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RenameDialogState {
    pub width: f32,
    pub height: f32,
    pub patterns: Vec<String>,
}

/// What the convert dialog opens on, carried between runs so converting a
/// second album is a click rather than the same four answers again. The
/// preset rides as its key (rox's `convert::Preset`), so an unknown one from
/// a newer build falls back to the default rather than failing the read. The
/// one key that isn't a preset is "custom", which sends the reader to the two
/// custom fields below it.
///
/// `ffmpeg` is the binary the conversion spawns. Empty means the one on
/// PATH, which is what almost every machine wants; a path here is for an
/// ffmpeg that isn't on it, and it's also the only thing in this struct
/// nothing in the app writes on its own.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ConvertSettings {
    pub preset: String,
    pub destination: Option<PathBuf>,
    pub pattern: String,
    /// The container a custom format writes, as a bare extension ("ogg").
    /// Only read when `preset` is the custom key.
    pub custom_ext: String,
    /// The ffmpeg output arguments a custom format runs, as typed. Split on
    /// whitespace where it's read, so there's no quoting in here.
    pub custom_args: String,
    /// Whether outputs mirror the library's folder shape rather than
    /// landing flat in the destination.
    pub mirror: bool,
    pub ffmpeg: String,
}

/// The stats window's remembered shape: size in logical pixels and the
/// range pick, written on close and when the range changes. The range
/// rides as the pick's key ("all", "year", "month"), decoded back in
/// rox's stats window; an unknown key falls back to all time.
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StatsWindowState {
    pub width: f32,
    pub height: f32,
    pub range: String,
}

/// The signals window's remembered shape: size in logical pixels, written
/// on close, and whether the page's explainer is unfolded, written when it
/// folds. An older file carrying only the size reads back with the
/// explainer open, which is where a first run starts anyway.
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

/// A window frame in logical pixels, plus whether the window was maximized
/// (the frame is then the restore size).
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(default)]
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
            look: LookState::default(),
            migrated: false,
            windows: WindowsState::default(),
            session: SessionState::default(),
            accounts: AccountsState::default(),
            library_roots: Vec::new(),
            library_root: None,
            watch_library: true,
            fold_case: false,
            split_genre_compounds: true,
            theme: Theme::default(),
            app_font_size: palette::FONT_SIZE_DEFAULT,
            icon_pack: None,
            restore_last_track: true,
            eq: EqSettings::default(),
            crossfade_secs: 0.0,
            crossfade_restore_secs: DEFAULT_CROSSFADE_SECS,
            crossfade_albums: false,
            replay_gain: ReplayGainSettings::default(),
            output: OutputSettings::default(),
            quit_to_tray: false,
            design_mode: true,
            resize_lock: false,
            check_updates: true,
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
            convert: ConvertSettings::default(),
            keymap: BTreeMap::new(),
        }
    }
}

impl Settings {
    /// Read the settings file, falling back to defaults if it is missing or
    /// unreadable. A corrupt file logs and resets rather than blocking start.
    pub fn load() -> Settings {
        let path = settings_path();
        // Parsed to a Value first, not straight to Settings: a pre-split file
        // carries the look flat beside the machine state, and the migration
        // below reads it back out of this same map.
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
        // A pre-split file carries all four shards flat beside the
        // preferences; back it up and drain its workspaces before any of them
        // read out of it.
        settings.migrated = raw.is_some() && Self::shard_missing();
        if let Some(text) = raw.as_deref() {
            if settings.migrated {
                Self::migrate_split(&value, text);
            }
        }
        settings.look = load_shard(&look_path(), "look", &value, LookState::from_legacy);
        settings.windows = load_shard(&windows_path(), "windows", &value, from_legacy);
        settings.session = load_shard(&session_path(), "session", &value, from_legacy);
        settings.accounts = load_shard(&accounts_path(), "accounts", &value, from_legacy);
        // A hand-edited volume seeds the engine's atomics directly, so the
        // engine's clamp range applies here too.
        settings.session.volume = if settings.session.volume.is_finite() {
            settings.session.volume.clamp(0.0, 2.0)
        } else {
            1.0
        };
        let appearance = &mut settings.look.bundle.appearance;
        // The transparency pair reads straight into color math, so
        // hand-edited values clamp to the unit range.
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
        // The frame knobs feed div sizes straight, so a hand-edited file
        // clamps each to its ceiling.
        appearance.frame = appearance.frame.clamped();
        // The threshold reads straight into the scrobble math and the
        // marker paint, so a hand-edited value clamps to a sane band.
        let lastfm = &mut settings.accounts.lastfm;
        lastfm.threshold = if lastfm.threshold.is_finite() {
            lastfm.threshold.clamp(0.1, 1.0)
        } else {
            0.5
        };
        // A file from before sessions were filed by api key carries one
        // flat session; it lands unattributed here and the next save
        // drops the flat pair.
        lastfm.fold_legacy_session();
        // The restored frame reads straight into window Bounds on open: a
        // non-finite field drops back to the centered default, and the size
        // floors at the window minimum so a zero or negative frame can't
        // open an invisible window. Negative origins are real on
        // multi-monitor setups, so finite ones stand.
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
        // A file from before multi-folder carries one library_root; it
        // seeds the list here and the next save drops it.
        if settings.library_roots.is_empty() {
            if let Some(root) = settings.library_root.take() {
                settings.library_roots.push(root);
            }
        }
        settings
    }

    /// Whether any shard file has yet to be written, the signal that the
    /// settings file still holds the pre-split shape.
    fn shard_missing() -> bool {
        [look_path(), windows_path(), session_path(), accounts_path()]
            .iter()
            .any(|path| !path.exists())
    }

    /// The one-shot move off the single-file format: keep a copy of the file
    /// as it stood, then write each workspace it holds out to its own file.
    /// The shards themselves need no move, since each reads its own fields
    /// straight out of the old flat map. Guarded to once per process, and the
    /// drain leaves a workspace already on disk alone, so a crash between here
    /// and the first save can't duplicate one on the next launch.
    fn migrate_split(value: &serde_json::Value, raw: &str) {
        // Checked before the one-shot guard: a load that beats the sink into
        // place leaves the move for the next one rather than burning it.
        let Some(migrate) = WORKSPACE_MIGRATOR.get() else {
            return;
        };
        static DONE: AtomicBool = AtomicBool::new(false);
        if DONE.swap(true, Ordering::Relaxed) {
            return;
        }
        let backup = settings_path().with_extension("json.bak-presplit");
        if !backup.exists() {
            if let Err(e) = std::fs::write(&backup, raw) {
                log::warn!("settings: backing up to {}: {e}", backup.display());
            }
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

    /// Change some fields and persist: reload the files, apply, write them
    /// back. Writers hold their own in-memory copies for reads, so going
    /// through the files here is what keeps one writer's save from
    /// reverting another's fields.
    pub fn update(f: impl FnOnce(&mut Settings)) {
        // Serialize the load-modify-save so a background writer (the update
        // check) and a UI-thread writer can't both read the same file, each
        // apply their own field, and have the last save drop the other's
        // change. The lock only spans the read-modify-write here.
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut settings = Settings::load();
        // Most writes touch one file: a volume nudge shouldn't rewrite every
        // dock dump, and a layout drag shouldn't rewrite the Last.fm session.
        // Compare each across the edit and write only what moved.
        let before = settings.prints();
        f(&mut settings);
        settings.save_changed(&before);
    }

    /// Each file's serialized contents, the before-and-after a write compares.
    fn prints(&self) -> Shards {
        Shards {
            core: serde_json::to_string(self).ok(),
            look: serde_json::to_string(&self.look).ok(),
            windows: serde_json::to_string(&self.windows).ok(),
            session: serde_json::to_string(&self.session).ok(),
            accounts: serde_json::to_string(&self.accounts).ok(),
        }
    }

    /// Write every file whose contents moved since `before`, plus any that
    /// isn't on disk yet (a fresh install, or the first save after the split).
    ///
    /// The writes aren't atomic across files: a crash partway leaves one of
    /// them an edit behind the others. Each is independent enough that this
    /// costs a repaint's worth of drift and never a corrupt file.
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

    /// The dark theme's user palette, its map over the defaults.
    pub fn palette_dark(&self) -> Palette {
        Palette::from_map(&self.look.bundle.palette_dark)
    }

    /// The light theme's user palette, its map over the designed light
    /// ladder.
    pub fn palette_light(&self) -> Palette {
        Palette::from_map_over(Palette::light(), &self.look.bundle.palette_light)
    }

    /// The stored palette map for a theme side, where the editor's edits
    /// land.
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

    /// A settings object wearing a look worth capturing, the source every
    /// bundle test snapshots from.
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

    /// A bundle must survive the file trip and land back on a fresh settings
    /// intact, or a shared workspace drifts on every hop.
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
        // The theme pick is the user's alone; a bundle never moves it.
        assert!(dst.theme == Theme::default());
        // Nor the font size: a readability choice, not a look to hand around.
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

    /// The bundle carries only the look, never machine- or account-bound
    /// state, so a shared file can't drag another install's folders or
    /// Last.fm session along.
    #[test]
    fn workspace_bundle_omits_machine_state() {
        let bundle = WorkspaceBundle::from_settings("mine".into(), &Settings::default());
        let json = serde_json::to_string(&bundle).unwrap();
        assert!(!json.contains("library_root"));
        assert!(!json.contains("lastfm"));
        assert!(!json.contains("session_key"));
        assert!(!json.contains("last_track"));
    }

    /// The point of filing sessions by api key: two installs signing with
    /// their own identities read the same file and each find their own.
    /// Last.fm binds a session to the key that authorized it, so a build
    /// picking up another's would be handed a refusal on every call.
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

        // Connecting the second install leaves the first alone, which is
        // what makes moving between them a one-time cost each.
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

    /// A file from before the split carries one session with nothing
    /// saying who minted it, so every build may try it and the one whose
    /// call lands keeps it.
    #[test]
    fn the_unattributed_session_goes_to_whoever_proves_it_works() {
        let mut lastfm: Lastfm = serde_json::from_value(serde_json::json!({
            "session_key": "sk-old",
            "username": "zealsprince",
        }))
        .unwrap();
        lastfm.fold_legacy_session();
        // Unclaimed, so either build reaches it.
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

    /// A refusal has to be recorded, not just acted on. Without the empty
    /// entry the build would reach for the unattributed session again on
    /// the next launch, and every launch after that.
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
        // And the build it does belong to still has it.
        assert_eq!(
            lastfm.session("nix-key").map(|s| s.key.as_str()),
            Some("sk-old")
        );

        // The record survives the file trip, or the next launch asks again.
        let back: Lastfm = serde_json::from_str(&serde_json::to_string(&lastfm).unwrap()).unwrap();
        assert!(back.session("release-key").is_none());
    }

    /// Disconnecting has to hold even where an unattributed session is
    /// sitting behind this build's own: the account came off screen, and
    /// a fallback that quietly put it back would read as connected again.
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

    /// A build with no identity of its own can't sign anything, so it
    /// holds no session however the file reads.
    #[test]
    fn no_api_key_means_no_session() {
        let mut lastfm = Lastfm::default();
        lastfm.connect("nix-key", "sk-nix".into(), "zealsprince".into());
        assert!(lastfm.session("").is_none());
        assert_eq!(lastfm.username(""), "");

        // And it can't file a refusal either, which would land in the
        // unattributed slot and take a carried-over session with it.
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

    /// The pool is what makes a named shader mean anything, so it has to
    /// survive the file trip with its sources intact. The bookmarks don't
    /// travel: they're the one part that means nothing on the machine this
    /// lands on.
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

        // A pool entry keeps its bookmark while it's the live look; only the
        // export scrub takes it off.
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

    /// A pool entry that no longer parses costs that entry and nothing else,
    /// the lenient rule every other list in the bundle follows.
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

    /// A plate a shader samples travels the way its source does, byte for
    /// byte, and the scrub doesn't reach into it: assets carry no paths, so
    /// there's nothing local in one to take off. A pool with no assets
    /// writes no key, which is every look that exists today.
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

    /// A hand-mangled asset costs that asset and nothing else, the same
    /// lenient rule the pool itself follows.
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

        // Data that isn't base64 at all reads out rather than panicking, so
        // the failure lands in a shader readout like every other one.
        let bad = ShaderAsset {
            file: "plate.png".to_string(),
            data: "not base64!".to_string(),
        };
        assert!(bad.decode().is_err());
    }

    /// The card and the screen shader ride the bundle, and a look that has
    /// neither writes neither key, so no existing workspace file grows a
    /// line it didn't have.
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

        // A bundle file written before any of this parses as a look with no
        // pool, no card, and no screen shader, which is what it was.
        let older: WorkspaceBundle =
            serde_json::from_value(serde_json::json!({ "version": 1, "name": "old" })).unwrap();
        assert!(older.shaders.is_empty());
        assert!(older.meta.is_empty());
        assert!(older.post_shader.is_none());
    }

    /// `created` is the day the workspace first existed, so it's written once
    /// and left alone; `updated` moves on every export.
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

        // A card is only empty while every field is, so one filled line keeps
        // the whole thing in the file.
        let described = WorkspaceMeta {
            description: "Warm and quiet.".to_string(),
            ..WorkspaceMeta::default()
        };
        assert!(!described.is_empty());
    }

    /// Saving over a workspace keeps the card the old one carried: the
    /// author's name and their notes survive, the day it was first made
    /// survives, and today's stamp stays on `updated`. A live look that
    /// carries its own card wins field by field, so a fork doesn't come out
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

        // The everyday overwrite: the live look carries nothing but today's
        // stamp, so the whole card comes back.
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

        // A look with its own card only fills the gaps.
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

        // Nothing to carry leaves the fresh card exactly as it was.
        let mut alone = WorkspaceMeta::default();
        alone.stamp("2026-08-07");
        alone.carry_forward(&WorkspaceMeta::default());
        assert_eq!(alone.created, "2026-08-07");
        assert!(alone.author.is_empty());
    }

    /// An export dates itself, so a shared file says when it was made
    /// without the author having to type it.
    #[test]
    fn from_settings_stamps_the_card() {
        let bundle = WorkspaceBundle::from_settings("mine".into(), &Settings::default());
        let today = utc_today();
        assert_eq!(bundle.meta.created, today);
        assert_eq!(bundle.meta.updated, today);
        // Ten characters of digits and hyphens, since a reader elsewhere
        // parses this as a date.
        assert_eq!(today.len(), 10);
        assert!(today.chars().all(|c| c.is_ascii_digit() || c == '-'));
    }

    /// The screen shader an export captures is the one the machine is
    /// wearing, since it lives in the settings rather than in the look. It
    /// arrives inlined and with its bookmark gone, the same way a pool entry
    /// does, and a machine that has never set one up exports no shader at
    /// all rather than a disabled placeholder.
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

        // A pass that's off but points somewhere still travels: it's set up,
        // and the look it belongs to is the one that decides when it runs.
        let parked = Settings {
            post_shader: PostShaderConfig {
                path: Some(file.clone()),
                ..PostShaderConfig::default()
            },
            ..Settings::default()
        };
        assert!(WorkspaceBundle::from_settings("mine".into(), &parked)
            .post_shader
            .is_some());

        assert!(
            WorkspaceBundle::from_settings("mine".into(), &Settings::default())
                .post_shader
                .is_none(),
            "an untouched default is nothing to carry"
        );

        std::fs::remove_file(&file).ok();
    }

    /// A shader ejects to a file named after the workspace and the entry,
    /// both folded through the filename sanitizer, so a pool entry called
    /// "Grain / Fine" lands somewhere instead of writing into a folder
    /// nobody asked for. A look with no name of its own is the one you're
    /// editing, which ejects under `_local`.
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
        // Pure punctuation empties out on both halves rather than writing a
        // hidden folder or a file with no name.
        assert_eq!(
            shader_eject_path_in(root, "...", "..."),
            root.join("_local").join("shader.wgsl")
        );
        assert_eq!(safe_file_stem("  padded  ", "fallback"), "padded");
        assert_eq!(safe_file_stem(".hidden", "fallback"), "hidden");
    }

    /// The trust pass reads sources out of a dump the same two places the
    /// scrub takes bookmarks out of, or a shipped look's panels would come
    /// up blank waiting for an approval nobody can give.
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

    /// A dump that wears a shader says so, whichever of the two shapes it is,
    /// and a pool name counts the same as inline text: that's the one a
    /// promoted shader leaves behind.
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
        // The Shader panel is one by definition, and a config saying nothing
        // runs the shipped example.
        assert!(dump_wears_shader(&serde_json::json!({
            "panel_name": "shader",
            "info": { "panel": {}},
        })));
    }

    /// Applying a look without its shaders switches both shapes off and
    /// leaves everything else, the sources included: nothing paints, and
    /// what the look came with is still there to turn on.
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
        // The sources stay where they are. They're still code that arrived
        // with a bundle, so the trust walk goes on seeing them.
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

    /// A Shader panel that saved no config of its own runs the shipped
    /// example, so parking it has to write the switch where there was
    /// nothing to write it beside.
    #[test]
    fn stripping_parks_a_shader_panel_with_no_config() {
        let mut dump = serde_json::json!({ "panel_name": "shader" });
        strip_dump_shaders(&mut dump);
        assert!(!dump_wears_shader(&dump));
        assert_eq!(dump["info"]["panel"]["enabled"], false);
    }

    /// The screen shader's file gets pulled inline on the way out, since a
    /// path alone imports as a dead pass. A path that reads nothing leaves
    /// the source empty rather than failing the export.
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

        // An inline source already in hand is never overwritten by the file.
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

        // A file that's gone leaves an empty source, which is the same dead
        // pass the bundle would have carried anyway.
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

    /// The scrub targets the two shapes that carry a shader bookmark and
    /// leaves every other `path` in a dump alone, since a folder panel's
    /// root is a path too and it's none of the scrub's business.
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

    /// The pool cache is what a render path reads, so it answers by name and
    /// says when it moved. The rev is the whole point: a surface holds its
    /// resolution and checks one atomic instead of diffing a page of WGSL.
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

    /// What the build ships runs without anyone agreeing to it a second
    /// time, and that trust lives beside the machine's own list rather than
    /// in it, so it never lands in a session file.
    #[test]
    fn shader_approved_trusts_what_the_build_ships() {
        let print = "shipped-with-the-binary-not-a-real-hash";
        assert!(!shader_approved(print));
        trust_shipped([print.to_string()]);
        assert!(shader_approved(print));
        // The session's own list never learned it, so nothing persists.
        assert!(!APPROVED_SHADERS.read().unwrap().contains(print));
    }

    /// The frame knobs feed div sizes straight, so `clamped` holds each to its
    /// own ceiling and floors at zero. This is the sanitizer `load` runs over a
    /// hand-edited frame.
    #[test]
    fn frame_clamps_each_knob_to_its_ceiling() {
        let clamped = Frame {
            margin: Sides::all(MARGIN_MAX + 100.0),
            padding: Sides::all(-5.0),
            rounding: ROUNDING_MAX + 1.0,
            // A split knob is held side by side, not by its widest.
            border: Sides::all(1.0).with(palette::Side::Top, BORDER_MAX + 10.0),
        }
        .clamped();
        assert_eq!(clamped.margin, Sides::all(MARGIN_MAX));
        // A negative knob floors at zero, not its ceiling.
        assert_eq!(clamped.padding, Sides::ZERO);
        assert_eq!(clamped.rounding, ROUNDING_MAX);
        assert_eq!(
            clamped.border,
            Sides::all(1.0).with(palette::Side::Top, BORDER_MAX)
        );
    }

    /// A non-finite knob resets to zero rather than propagating NaN into a
    /// layout size.
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
        // Finite, in-range knobs stand.
        assert_eq!(clamped.rounding, 6.0);
        assert_eq!(clamped.border, Sides::all(2.0));
    }

    /// A round-trip through the JSON file format preserves the fields a
    /// settings write cares about, so nothing silently drops on save and
    /// reload.
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

    /// The post shader pick survives the file, and a file that predates the
    /// field reads as off with no path rather than failing the load.
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

    /// The screen shader's routes ride the same field, and a file written
    /// before they existed reads as none - which is what keeps the older
    /// pool-order feed running for anyone who never opens the editor.
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

        // An empty list writes nothing at all, so a settings file that was
        // never routed stays exactly as it was.
        let bare = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!bare.contains("routes"));

        let older: Settings = serde_json::from_str(r#"{"post_shader":{"enabled":true}}"#).unwrap();
        assert!(older.post_shader.routes.is_empty());
    }

    /// The measurement pass's destination survives the file, and an older
    /// file that predates the field reads as the database default rather
    /// than as permission to rewrite everyone's tags.
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

    /// The analysis pass's destination survives the file, and neither an
    /// older file that predates the field nor a newer file naming something
    /// this build never heard of reads as permission to rewrite everyone's
    /// tags. Both land on the database, which is the answer that touches
    /// nothing.
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

    /// The follow-the-watcher switch survives the file, and a file that
    /// predates it reads as off: measuring is an afternoon of decoding and in
    /// tags mode it rewrites files, so an upgrade never turns it on for you.
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

    /// Each of the three plain shards round-trips through its own file.
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
        accounts.lastfm.threshold = 0.8;
        accounts
            .lastfm
            .connect("api-key", "sk".into(), "zealsprince".into());
        let back: AccountsState =
            serde_json::from_str(&serde_json::to_string(&accounts).unwrap()).unwrap();
        assert_eq!(back.lastfm.threshold, 0.8);
        assert_eq!(back.lastfm.username("api-key"), "zealsprince");

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

    /// Nothing that lives in a file of its own rides the settings file: the
    /// look's dock dumps, the window frames, the volatile playback state, and
    /// above all the credentials, which is the whole point of the accounts
    /// file. The settings file is the one people are pointed at.
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
            // the windows. Quote-anchored: the post shader's all_windows
            // preference legitimately carries the substring, while a leaked
            // shard would appear as this exact key.
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

    /// The look round-trips through its own file, the other half of the
    /// split: what `workspace.json` holds comes back whole.
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

    /// A pre-split settings file carried the look flat beside the machine
    /// state. Reading one has to find every piece of it, or an upgrade loses
    /// the user's layouts, palette, and appearance in one go.
    #[test]
    fn legacy_settings_yields_its_look() {
        // The shape the old single file wrote: look keys as top-level
        // siblings of the machine state.
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
        // The appearance knobs sat flat too, so they need their own pass.
        let a = &look.bundle.appearance;
        assert_eq!(a.surface_opacity, 0.5);
        assert_eq!(a.frame.rounding, 12.0);
        assert!(!a.seams);
        assert!(a.art_theming);
        assert!(a.rating_style == RatingStyle::Numeric);
        assert!(a.hide_menubar);
        assert!(!a.os_decorations);
        // A pre-split file names no workspace: the look is whatever it was
        // edited into, not one you can point at.
        assert!(look.bundle.name.is_empty());
    }

    /// A fresh install has no settings file at all, which reads as the
    /// default look rather than anything half-migrated.
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

    /// Unknown fields drop and missing ones take defaults, so every file
    /// survives version drift in both directions rather than failing to load.
    #[test]
    fn settings_deserialize_tolerates_drift() {
        // A field the current build never wrote, plus a subset of real ones.
        let json = r#"{ "fold_case": true, "some_future_knob": 42, "quit_to_tray": true }"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert!(s.fold_case);
        assert!(s.quit_to_tray);
        // A field absent from the file falls back to its default.
        assert!(s.watch_library);
        assert!(s.theme == Theme::default());
        // Design mode in particular: every settings file written before it
        // existed is missing the key, and those installs must keep the
        // editing controls they have always had rather than lose them to a
        // default of off.
        assert!(s.design_mode);

        // And the same for a shard, where a missing volume must not read as
        // silence.
        let session: SessionState = serde_json::from_str(r#"{ "muted": true }"#).unwrap();
        assert!(session.muted);
        assert_eq!(session.volume, SessionState::default().volume);
        assert!(session.loop_mode() == LoopMode::Off);
    }

    /// The loop mode rides its file as a wire name, so the engine's enum stays
    /// serde-free. An unrecognized value degrades rather than erroring.
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

    /// A pre-split file carried the windows, session, and accounts flat
    /// alongside everything else. Each reads straight back out of that map
    /// because every field kept its name, and the window fields that did get
    /// renamed carry an alias for the one they had.
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
        assert_eq!(accounts.lastfm.threshold, 0.8);
        assert!(accounts.discord.enabled);

        // The renamed window fields come across on their aliases.
        let windows: WindowsState = from_legacy(&json);
        assert_eq!(windows.main.map(|w| w.width), Some(800.0));
        assert_eq!(windows.stats.map(|s| s.range), Some("year".to_string()));
        assert_eq!(windows.console.map(|s| s.width), Some(700.0));
        assert_eq!(windows.panel_settings.map(|s| s.width), Some(640.0));
        assert_eq!(windows.settings.map(|s| s.width), Some(900.0));
        assert!(windows.queue_view.is_some());
    }

    /// One sub-object short of a field must not cost the whole shard. An old
    /// file written before a field existed is exactly what a migration reads,
    /// and without a default on the nested type the miss fails the shard and
    /// takes every unrelated value in it: volume, loop mode, last scan.
    #[test]
    fn a_short_sub_object_costs_only_itself() {
        let json = serde_json::json!({
            "volume": 0.3,
            "shuffle": true,
            "last_scan": 12345,
            // No "url": the shape an older build wrote.
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

        // Same for a window frame missing the flag that came later.
        let json = serde_json::json!({
            "window": { "x": 1.0, "y": 2.0, "width": 800.0, "height": 600.0 },
        });
        let windows: WindowsState = from_legacy(&json);
        assert_eq!(windows.main.map(|w| w.height), Some(600.0));
    }

    /// One broken preset costs that preset. Without the lenient list it fails
    /// the whole `layouts` array, which fails the look, which resets
    /// `workspace.json` to defaults: every other preset, the palette, and the
    /// appearance gone over one entry missing its dump.
    #[test]
    fn a_broken_preset_costs_only_that_preset() {
        let json = serde_json::json!({
            "layouts": [
                { "name": "good", "dump": { "k": "v" } },
                // No dump: the shape a truncated write or a hand-edit leaves.
                { "name": "broken" },
                { "name": "also good", "dump": { "k": "v2" } },
            ],
            "primary_layout": "good",
            "palette_dark": { "accent": "#336699" },
        });
        let bundle: WorkspaceBundle = serde_json::from_value(json).unwrap();
        let names: Vec<&str> = bundle.layouts.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["good", "also good"]);
        // The rest of the look is untouched, which is the whole point.
        assert_eq!(bundle.primary_layout.as_deref(), Some("good"));
        assert_eq!(
            bundle.palette_dark.get("accent").map(String::as_str),
            Some("#336699")
        );
    }

    /// Same for the per-layout working copies, keyed by name rather than
    /// ordered, and for the signal pool a look's routes ride.
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

    /// The approved shader hashes ride the session file: machine-local, so a
    /// copied settings file carries none of them, and absent from a file
    /// nobody has approved anything on.
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
        // A set, sorted, so two approvals in the other order don't rewrite
        // the file and the diff stays readable.
        assert_eq!(
            written["approved_shaders"],
            serde_json::json!(["beef", "cafe"])
        );

        // A file written before the gate existed loads clean, approving
        // nothing.
        let older: SessionState =
            serde_json::from_value(serde_json::json!({ "volume": 0.4 })).expect("read");
        assert!(older.approved_shaders.is_empty());

        // The workspace bundle is what a shared look travels as; the trust
        // list is not in it, and can't be.
        let bundle = serde_json::to_value(WorkspaceBundle::default()).expect("dump");
        assert!(bundle.get("approved_shaders").is_none());
    }

    /// The live list and the file stay in step, and approving the same hash
    /// twice is a no-op rather than a second write.
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

    /// A queue that no longer parses reads as no queue, and takes nothing with
    /// it. Dropping the bad entry instead would be worse than useless: the
    /// cursor indexes the entries, so a short list resumes the wrong track.
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
        // Everything that has nothing to do with the queue survives it.
        assert_eq!(session.volume, 0.4);
        assert!(session.loop_mode() == LoopMode::All);
        assert!(session.shuffle);
        assert_eq!(session.last_scan, 12345);
    }

    /// The saved queue carries each entry's subsong, so a whole-disc rip comes
    /// back as its own tracks instead of the image over and over. A file
    /// written before cue support carries no `sub` at all and has to keep
    /// reading, as every track in it was a plain file.
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

        // An older file: no `sub` anywhere, on either shape.
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

    /// A shuffle mode this build has never heard of reads as Random and takes
    /// nothing with it. Failing instead would cost the whole session shard: the
    /// volume, the loop mode, the saved queue, and `last_scan`, which is a full
    /// library rescan over one word a newer build wrote.
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

        // A mode this build does know still reads as itself, and so does one
        // written as something that was never a mode at all.
        let session: SessionState =
            serde_json::from_value(serde_json::json!({ "shuffle_mode": "similar" })).unwrap();
        assert_eq!(session.shuffle_mode, ShuffleMode::Similar);
        let session: SessionState =
            serde_json::from_value(serde_json::json!({ "shuffle_mode": 7 })).unwrap();
        assert_eq!(session.shuffle_mode, ShuffleMode::Random);
    }

    /// Every other closed set of words in the shards reads the same way, and
    /// the blast radius is worse in each of them than in the session: the theme
    /// sits beside the library folders, the rating style beside the palette,
    /// and the lyrics destination beside the Last.fm session key.
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

    /// A window shape that no longer parses costs that window's remembered
    /// size, not every window's.
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

    /// The stamp is what separates a weights file that was rewritten in place
    /// from one that's only being picked again, so it has to move when the
    /// bytes do. Without that, a retrained checkpoint's vectors land under the
    /// id the previous one was hashed to.
    #[test]
    fn a_rewritten_weights_file_stamps_differently() {
        let dir = std::env::temp_dir().join(format!("rox-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("weights.safetensors");
        std::fs::write(&path, b"one checkpoint").unwrap();
        let first = file_stamp(&path).unwrap();
        std::fs::write(&path, b"a different checkpoint").unwrap();
        assert_ne!(file_stamp(&path), Some(first));
        // Neither a folder nor a path with nothing at it is a checkpoint.
        assert_eq!(file_stamp(&dir), None);
        assert_eq!(file_stamp(&dir.join("gone")), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A migrated load rewrites its files even though the edit moved nothing.
    /// Skipping the write is right in steady state, and wrong exactly once:
    /// the settings file is sitting there in the pre-split shape, a no-op edit
    /// serializes to the same bytes either way, and without the force the old
    /// flat keys, credentials among them, would never be stripped.
    #[test]
    fn a_migrated_load_rewrites_an_unmoved_file() {
        let dir = std::env::temp_dir().join(format!("rox-shard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shard.json");
        // The file on disk still holds the old shape.
        std::fs::write(&path, r#"{"stale": true}"#).unwrap();
        let print = serde_json::to_string(&SessionState::default()).ok();

        // Unmoved and present: normally nothing to do.
        write_shard(
            path.clone(),
            "shard",
            &print,
            &print,
            false,
            &SessionState::default(),
        );
        assert!(std::fs::read_to_string(&path).unwrap().contains("stale"));

        // Same edit, but the load came out of a pre-split file.
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
}
