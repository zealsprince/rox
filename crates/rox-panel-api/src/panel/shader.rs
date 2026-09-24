//! Per-panel surface shaders: a WGSL fragment stage over a panel's body
//! rect, layered under the app-wide post shader. The config lives on
//! [`PanelChrome`](super::PanelChrome), so persistence and bundles come
//! free; the render side is [`PanelSurface`], recorded by the
//! [`Themed`](super::themed) wrapper after the body paints.
//!
//! Also shared with the Shader panel: slot targets, the `// @slot n:`
//! labels, and the eight `meta` floats.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};

use gpui::{App, Bounds, EntityId, Global, Pixels, UserShaderId, WeakEntity, Window, WindowId};
use serde::{Deserialize, Serialize};

use rox_design::palette::Sides;
use rox_viz::signal::{Route, SignalHub};

use crate::signal_ui::{self, RouteTargets};
use rox_services::player::Player;

use super::{AppState, PanelChrome};

mod chain;
mod cursor;
pub mod edit;

pub use chain::{
    AssetImage, AssetRef, COVER_SOURCE, ChainSpec, PassSpec, ProgramCtx, fallback_cover,
    parse_chain, register_program, resolve_assets, uses_cover, uses_mask, validate_frame_pass,
    validate_program,
};
pub use cursor::{CURSOR_FADE, CURSOR_HOLD, cursor_presence, reads_cursor, watch_cursor};

/// The uniform block's width.
pub const SLOTS: usize = 16;

/// The builtins, trusted by the approval gate by construction. Each shows a
/// different part of the contract, so together they're the authoring
/// reference.
pub const PLASMA: &str = include_str!("shader/plasma.wgsl");
pub const TRAILS: &str = include_str!("shader/trails.wgsl");
pub const SHEEN: &str = include_str!("shader/sheen.wgsl");
pub const SHADOW: &str = include_str!("shader/shadow.wgsl");
pub const COVER: &str = include_str!("shader/cover.wgsl");
pub const BADGE: &str = include_str!("shader/badge.wgsl");
pub const LAMP: &str = include_str!("shader/lamp.wgsl");
pub const CUBE: &str = include_str!("shader/cube.wgsl");
pub const BLOOM: &str = include_str!("shader/bloom.wgsl");
pub const TUBE: &str = include_str!("shader/tube.wgsl");

/// One shipped example. Overlay-ness isn't a field: it's an `// @overlay`
/// line in the source, read by [`overlay`], because a pool shader has no
/// struct to hold a flag.
pub struct Preset {
    /// Untranslated: these read as product names.
    pub label: &'static str,
    /// A message key, resolved by [`pick_blurb`].
    pub blurb: &'static str,
    pub source: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        label: "Plasma",
        blurb: "shader-blurb-plasma",
        source: PLASMA,
    },
    Preset {
        label: "Trails",
        blurb: "shader-blurb-trails",
        source: TRAILS,
    },
    Preset {
        label: "Sheen",
        blurb: "shader-blurb-sheen",
        source: SHEEN,
    },
    Preset {
        label: "Shadow",
        blurb: "shader-blurb-shadow",
        source: SHADOW,
    },
    Preset {
        label: "Cover",
        blurb: "shader-blurb-cover",
        source: COVER,
    },
    Preset {
        label: "Badge",
        blurb: "shader-blurb-badge",
        source: BADGE,
    },
    Preset {
        label: "Lamp",
        blurb: "shader-blurb-lamp",
        source: LAMP,
    },
    Preset {
        label: "Cube",
        blurb: "shader-blurb-cube",
        source: CUBE,
    },
    Preset {
        label: "Bloom",
        blurb: "shader-blurb-bloom",
        source: BLOOM,
    },
    Preset {
        label: "Tube",
        blurb: "shader-blurb-tube",
        source: TUBE,
    },
];

/// How often a watched source file gets stat'd while its surface draws.
pub const RELOAD_EVERY: Duration = Duration::from_millis(500);

/// A panel's surface shader as it persists. The source is stored inline,
/// since a bundle holding only an absolute path would import dead on
/// anyone else's machine. The path is only a bookmark for reloads.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelShader {
    pub enabled: bool,
    /// A `fs_user(uv)` definition, plus whatever it calls.
    pub source: String,
    /// A pool name. Set, the pool's copy runs and the inline source is
    /// ignored; see [`resolve_source`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub path: Option<PathBuf>,
    pub routes: Vec<Route>,
    /// Hand-set slot values. A route on the same slot wins while it's there.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub manual: Vec<(u8, f32)>,
    /// Keep requesting frames with the hub silent. Off, a paused player's
    /// shader freezes, bar a cursor reader's own fade frames.
    pub run_when_idle: bool,
}

impl Default for PanelShader {
    fn default() -> Self {
        PanelShader {
            enabled: true,
            source: String::new(),
            name: None,
            path: None,
            routes: Vec::new(),
            manual: Vec::new(),
            run_when_idle: false,
        }
    }
}

impl PanelShader {
    /// A name counts even if the pool doesn't hold it; that's for
    /// [`resolve_source`] to answer at registration.
    pub fn runnable(&self) -> bool {
        self.enabled && (self.name.is_some() || !self.source.trim().is_empty())
    }
}

/// The WGSL a shader config actually runs. A name wins outright, and a name
/// the pool doesn't hold gives None. Never fall through to the inline copy:
/// stale text running under the name is worse than a blank panel.
///
/// Not per frame: it takes a lock and copies a page of text. A cached
/// answer goes stale when
/// [`shader_pool_rev`](rox_core::settings::shader_pool_rev) moves.
pub fn resolve_source(name: Option<&str>, inline: &str) -> Option<String> {
    match name {
        Some(name) => rox_core::settings::shader_pool_get(name).map(|shader| shader.source),
        None => (!inline.trim().is_empty()).then(|| inline.to_string()),
    }
}

/// Which entry of a shader picker a config currently matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// Nothing to run.
    Empty,
    /// `missing` when the workspace doesn't hold that name anymore.
    Named { name: String, missing: bool },
    /// A file on this machine, watched for saves.
    File(PathBuf),
    /// An index into [`PRESETS`].
    Example(usize),
    /// A source of its own that matches nothing else.
    Custom,
}

/// `resolved` is what actually runs. A file bookmark beats an example
/// match: an ejected example is text-identical until the first edit, and
/// the picker has to offer Reload rather than a second eject.
pub fn pick(name: Option<&str>, path: Option<&Path>, resolved: Option<&str>) -> Pick {
    if let Some(name) = name {
        return Pick::Named {
            name: name.to_string(),
            missing: resolved.is_none(),
        };
    }
    let source = resolved.unwrap_or_default().trim();
    if source.is_empty() {
        return Pick::Empty;
    }
    if let Some(path) = path {
        return Pick::File(path.to_path_buf());
    }
    match PRESETS
        .iter()
        .position(|preset| preset.source.trim() == source)
    {
        Some(index) => Pick::Example(index),
        None => Pick::Custom,
    }
}

/// A file shows its stem; the note under the row spells out the path.
pub fn pick_label(pick: &Pick) -> String {
    match pick {
        Pick::Empty => rox_i18n::t!("shader-pick-none").to_string(),
        Pick::Named {
            name,
            missing: true,
        } => rox_i18n::t!("shader-pick-missing", name = name.clone()).to_string(),
        Pick::Named { name, .. } => name.clone(),
        Pick::File(path) => path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .filter(|stem| !stem.is_empty())
            .unwrap_or_else(|| path.display().to_string()),
        Pick::Example(index) => PRESETS
            .get(*index)
            .map(|preset| preset.label.to_string())
            .unwrap_or_else(|| rox_i18n::t!("shader-pick-custom").to_string()),
        Pick::Custom => rox_i18n::t!("shader-pick-custom").to_string(),
    }
}

pub fn pick_blurb(index: usize) -> gpui::SharedString {
    PRESETS
        .get(index)
        .map(|preset| rox_i18n::t!(preset.blurb))
        .unwrap_or_default()
}

/// A source's identity in the approved list: hex SHA-256 of the trimmed
/// text, so an editor's trailing newline isn't a different program.
pub fn fingerprint(source: &str) -> String {
    use sha2::{Digest as _, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(source.trim().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Builtins are approved by construction: they came with the binary.
pub fn builtin(source: &str) -> bool {
    let source = source.trim();
    PRESETS.iter().any(|preset| preset.source.trim() == source)
}

/// Whether this source may run on this machine.
///
/// Shaders travel in layouts and bundles as inline WGSL, so applying
/// somebody else's look hands rox somebody else's code. Nothing registers
/// until its hash is in the machine-local approved list, and only a direct
/// user action writes to it: a file pick, a reload, a preset, or Approve.
pub fn approved(source: &str) -> bool {
    source.trim().is_empty()
        || builtin(source)
        || rox_core::settings::shader_approved(&fingerprint(source))
}

/// Only call where the user themselves put the source there. Never from
/// the layout apply or restore side.
pub fn approve(source: &str) {
    if source.trim().is_empty() || builtin(source) {
        return;
    }
    rox_core::settings::approve_shader(&fingerprint(source));
}

const EJECT_VARIANTS: u32 = 99;

/// The live look's name, the folder ejected shaders go into. Unsaved looks
/// have none; the path helper turns that into `_local`, so pass it as is.
pub fn live_workspace() -> String {
    rox_core::settings::Settings::load().look.bundle.name
}

/// Write a shader out under the live workspace's folder for an external
/// editor. An existing file is only overwritten when it holds the same
/// shader, hash for hash; otherwise the eject slides down to `name-2` and
/// on, so diverged edits never get clobbered.
pub fn eject(name: &str, source: &str) -> std::io::Result<PathBuf> {
    eject_in(
        &rox_core::settings::shaders_dir(),
        &live_workspace(),
        name,
        source,
        &[],
    )
}

/// [`eject`] under a given root, with the shader's images. Images are
/// overwritten rather than slid to a variant: the shader binds them by file
/// name, so a numbered copy would never be sampled.
pub fn eject_in(
    root: &Path,
    workspace: &str,
    name: &str,
    source: &str,
    assets: &[rox_core::settings::ShaderAsset],
) -> std::io::Result<PathBuf> {
    let print = fingerprint(source);
    for variant in 1..=EJECT_VARIANTS {
        let stem = if variant == 1 {
            name.to_string()
        } else {
            format!("{name}-{variant}")
        };
        let path = rox_core::settings::shader_eject_path_in(root, workspace, &stem);
        // Only a file holding a different shader is skipped. A failed read
        // falls through so the write reports the real error.
        if std::fs::read_to_string(&path).is_ok_and(|text| fingerprint(&text) != print) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, source)?;
        write_assets(path.parent(), assets)?;
        return Ok(path);
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        rox_i18n::t!(
            "shader-eject-name-taken",
            name = name.to_string(),
            count = EJECT_VARIANTS as u64
        )
        .to_string(),
    ))
}

/// Write a shader's images out beside its `.wgsl`.
fn write_assets(
    dir: Option<&Path>,
    assets: &[rox_core::settings::ShaderAsset],
) -> std::io::Result<()> {
    let Some(dir) = dir else {
        return Ok(());
    };
    for asset in assets {
        let bytes = asset
            .decode()
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        std::fs::write(dir.join(&asset.file), bytes)?;
    }
    Ok(())
}

/// Eject a pool entry and bookmark the file on it, so [`poll_pool`] brings
/// edits back to every panel using the name.
pub fn eject_pool_entry(name: &str) -> std::io::Result<PathBuf> {
    let Some(entry) = rox_core::settings::shader_pool_get(name) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            rox_i18n::t!("shader-eject-not-in-pool", name = name.to_string()).to_string(),
        ));
    };
    let path = eject_in(
        &rox_core::settings::shaders_dir(),
        &live_workspace(),
        name,
        &entry.source,
        &entry.assets,
    )?;
    let mut pool = rox_core::settings::shader_pool();
    if let Some(entry) = pool.iter_mut().find(|entry| entry.name == name) {
        entry.path = Some(path.clone());
    }
    rox_core::settings::set_shader_pool(pool);
    Ok(path)
}

/// The panel's label, or a short source hash when it has none.
pub fn eject_name(label: &str, source: &str) -> String {
    let label = label.trim();
    if label.is_empty() {
        format!("shader-{}", &fingerprint(source)[..8])
    } else {
        label.to_string()
    }
}

/// Put an inline shader into the pool under `name`, returning whether it
/// replaced an entry. Promoting approves, the same as picking a file.
///
/// `path` replaces the entry's bookmark. Never keep a replaced entry's old
/// one: that file holds the old shader and the pool watch would pull it
/// back over the save. Images beside `path` travel with the entry.
pub fn save_to_pool(name: &str, source: &str, path: Option<PathBuf>) -> bool {
    approve(source);
    let captured = sibling_assets(source, path.as_deref());
    let mut pool = rox_core::settings::shader_pool();
    let replaced = match pool.iter_mut().find(|entry| entry.name == name) {
        Some(entry) => {
            entry.source = source.to_string();
            entry.path = path;
            // Additive: disk wins per file, and held plates stay while
            // their `@asset` line is being edited.
            for asset in captured {
                match entry.assets.iter_mut().find(|held| held.file == asset.file) {
                    Some(held) => *held = asset,
                    None => entry.assets.push(asset),
                }
            }
            true
        }
        None => {
            pool.push(rox_core::settings::NamedShader {
                name: name.to_string(),
                source: source.to_string(),
                path,
                assets: captured,
            });
            false
        }
    };
    rox_core::settings::set_shader_pool(pool);
    replaced
}

fn sibling_assets(source: &str, path: Option<&Path>) -> Vec<rox_core::settings::ShaderAsset> {
    let Some(dir) = path.and_then(Path::parent) else {
        return Vec::new();
    };
    let Ok(spec) = parse_chain(source) else {
        return Vec::new();
    };
    spec.assets
        .iter()
        .filter(|asset| !asset.is_cover())
        .filter_map(|asset| {
            let bytes = std::fs::read(dir.join(&asset.file)).ok()?;
            Some(rox_core::settings::ShaderAsset::from_bytes(
                asset.file.clone(),
                &bytes,
            ))
        })
        .collect()
}

/// The pool's hot reload, one for the app: a per-panel watch would stat
/// the same file per panel and race itself writing back. Taken with
/// `try_lock` since it's called from paint and the second panel through in
/// a frame has nothing to add.
static POOL_WATCH: LazyLock<Mutex<PoolWatch>> = LazyLock::new(|| Mutex::new(PoolWatch::default()));

#[derive(Default)]
struct PoolWatch {
    checked: Option<Instant>,
    /// An entry with no stamp yet reads its files once, so an edit made
    /// while rox was closed is picked up on open.
    stamps: HashMap<String, EntryStamps>,
}

/// Size and mtime of an entry's `.wgsl` and each image it declares.
#[derive(Default)]
struct EntryStamps {
    source: Option<(u64, i64)>,
    assets: HashMap<String, (u64, i64)>,
}

#[derive(Default)]
struct PoolEdits {
    /// Sources that came back changed, for the caller to approve.
    fresh: Vec<String>,
    /// An image edit changes no source but still has to be written back.
    changed: bool,
}

/// Stat the pool's bookmarked files and pull edits into their entries.
/// A changed source is approved: the user pointed rox at the file and
/// edited it. Writing the pool re-registers every panel using it.
pub fn poll_pool() {
    let Ok(mut watch) = POOL_WATCH.try_lock() else {
        return;
    };
    let now = Instant::now();
    if watch
        .checked
        .is_some_and(|last| now.duration_since(last) < RELOAD_EVERY)
    {
        return;
    }
    watch.checked = Some(now);
    let mut pool = rox_core::settings::shader_pool();
    let edits = pool_reload(&mut watch.stamps, &mut pool);
    // Release before touching settings files, so no other panel's paint
    // waits on the write.
    drop(watch);
    if !edits.changed {
        return;
    }
    for source in &edits.fresh {
        approve(source);
    }
    rox_core::settings::set_shader_pool(pool);
}

/// The stat-and-read half of the pool watch, over a pool handed in for
/// tests. Images to watch come off the source's `@asset` lines, so
/// declaring one and dropping the file beside the shader is one save.
fn pool_reload(
    stamps: &mut HashMap<String, EntryStamps>,
    pool: &mut [rox_core::settings::NamedShader],
) -> PoolEdits {
    let mut edits = PoolEdits::default();
    for entry in pool.iter_mut() {
        let Some(path) = entry.path.clone() else {
            continue;
        };
        let marks = stamps.entry(entry.name.clone()).or_default();
        // A vanished file leaves the entry alone. The stamp stays put, so
        // the file coming back reads as news.
        if let Some(stamp) = rox_core::settings::file_stamp(&path)
            && marks.source != Some(stamp)
        {
            marks.source = Some(stamp);
            if let Ok(text) = std::fs::read_to_string(&path)
                && text.trim() != entry.source.trim()
            {
                entry.source = text.clone();
                edits.fresh.push(text);
                edits.changed = true;
            }
        }
        let (Some(dir), Ok(spec)) = (path.parent(), parse_chain(&entry.source)) else {
            continue;
        };
        for asset in &spec.assets {
            if asset.is_cover() {
                continue;
            }
            let file = dir.join(&asset.file);
            let Some(stamp) = rox_core::settings::file_stamp(&file) else {
                continue;
            };
            if marks.assets.get(&asset.file) == Some(&stamp) {
                continue;
            }
            marks.assets.insert(asset.file.clone(), stamp);
            let Ok(bytes) = std::fs::read(&file) else {
                continue;
            };
            let fresh = rox_core::settings::ShaderAsset::from_bytes(asset.file.clone(), &bytes);
            match entry.assets.iter_mut().find(|held| held.file == asset.file) {
                Some(held) if held.data == fresh.data => {}
                Some(held) => {
                    *held = fresh;
                    edits.changed = true;
                }
                None => {
                    entry.assets.push(fresh);
                    edits.changed = true;
                }
            }
        }
    }
    // Drop departed entries' stamps, or the name coming back would treat
    // its file's next edit as old news.
    stamps.retain(|name, _| pool.iter().any(|entry| entry.name == *name));
    edits
}

/// The mtime watch behind hot reload, for both shader surfaces. Never
/// requests a frame of its own; it rides the paint the shader already asked
/// for.
#[derive(Default)]
pub struct SourceWatch {
    stamp: Option<(u64, i64)>,
    /// Unseeded, the first check reads the file whatever the stamp says, so
    /// an edit made while rox was closed is picked up on open.
    seeded: bool,
    checked: Option<Instant>,
}

impl SourceWatch {
    /// A watch for a source that was just read from `path`, so only the
    /// next edit wakes it.
    pub fn seeded(path: Option<&Path>) -> SourceWatch {
        SourceWatch {
            stamp: path.and_then(rox_core::settings::file_stamp),
            seeded: path.is_some(),
            checked: Some(Instant::now()),
        }
    }

    /// The file's contents when it has moved since the last look. A file
    /// that disappears leaves the running source alone and the watch armed.
    pub fn poll(&mut self, path: &Path) -> Option<String> {
        let now = Instant::now();
        if self
            .checked
            .is_some_and(|last| now.duration_since(last) < RELOAD_EVERY)
        {
            return None;
        }
        self.checked = Some(now);
        let stamp = rox_core::settings::file_stamp(path)?;
        if self.seeded && self.stamp == Some(stamp) {
            return None;
        }
        self.seeded = true;
        self.stamp = Some(stamp);
        std::fs::read_to_string(path).ok()
    }
}

pub fn slot_target(slot: usize) -> String {
    format!("slot{slot}")
}

pub fn target_slot(id: &str) -> Option<usize> {
    let slot: usize = id.strip_prefix("slot")?.parse().ok()?;
    (slot < SLOTS).then_some(slot)
}

/// Whether a shader declares, with a bare `// @overlay` line, that the app
/// stays usable under it (transparent, or printing `screen` through).
///
/// Declared because it can't be derived: binding `screen` doesn't prove the
/// frame gets through, and alpha is computed per pixel. A shader that
/// declares nothing is taken at its most disruptive.
pub fn overlay(source: &str) -> bool {
    source
        .lines()
        .any(|line| chain::directive(line, "@overlay").is_some())
}

/// The slot names a shader declares with `// @slot n: name` comments.
pub fn slot_labels(source: &str) -> Vec<Option<String>> {
    let mut labels = vec![None; SLOTS];
    for line in source.lines() {
        let Some(rest) = line.trim_start().strip_prefix("//") else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix("@slot") else {
            continue;
        };
        let Some((index, name)) = rest.trim_start().split_once(':') else {
            continue;
        };
        let (Ok(index), name) = (index.trim().parse::<usize>(), name.trim()) else {
            continue;
        };
        if index < SLOTS && !name.is_empty() {
            labels[index] = Some(name.to_string());
        }
    }
    labels
}

pub fn slot_label(labels: &[Option<String>], slot: usize) -> String {
    match labels.get(slot).and_then(|name| name.clone()) {
        Some(name) => name,
        None => format!("slot {slot}"),
    }
}

/// The shader's side of [`RouteTargets`]. The paint path builds these
/// unlabelled.
pub struct SlotTargets {
    pub slots: [f32; SLOTS],
    labels: Vec<Option<String>>,
}

impl Default for SlotTargets {
    fn default() -> Self {
        SlotTargets {
            slots: [0.0; SLOTS],
            labels: vec![None; SLOTS],
        }
    }
}

impl SlotTargets {
    pub fn labelled(source: &str) -> Self {
        SlotTargets {
            slots: [0.0; SLOTS],
            labels: slot_labels(source),
        }
    }
}

/// The WGSL accessor a slot arrives on, for the settings pages to show.
pub fn slot_accessor(slot: usize) -> String {
    let lane = ["x", "y", "z", "w"][slot % 4];
    format!("params.signals[{}].{lane}", slot / 4)
}

pub fn manual_value(manual: &[(u8, f32)], slot: usize) -> Option<f32> {
    manual
        .iter()
        .find(|(at, _)| *at as usize == slot)
        .map(|(_, value)| *value)
}

pub fn set_manual_value(manual: &mut Vec<(u8, f32)>, slot: usize, value: f32) {
    let value = value.clamp(0.0, 1.0);
    match manual.iter_mut().find(|(at, _)| *at as usize == slot) {
        Some(entry) => entry.1 = value,
        None => manual.push((slot as u8, value)),
    }
}

/// Run before the routes resolve, so a route wins over a hand-set value.
pub fn seed_manual(targets: &mut SlotTargets, manual: &[(u8, f32)]) {
    for (slot, value) in manual {
        if let Some(entry) = targets.slots.get_mut(*slot as usize) {
            *entry = value.clamp(0.0, 1.0);
        }
    }
}

impl RouteTargets for SlotTargets {
    fn targets(&self) -> Vec<(String, String)> {
        (0..SLOTS)
            .map(|slot| (slot_target(slot), slot_label(&self.labels, slot)))
            .collect()
    }

    fn apply(&mut self, id: &str, value: f32) {
        if let Some(slot) = target_slot(id) {
            self.slots[slot] = value;
        }
    }
}

/// Each window's hub and player. Panels paint far from any `AppState`, so
/// the wrapper looks these up by window.
#[derive(Default)]
struct ShaderFeeds(HashMap<WindowId, Feed>);

impl Global for ShaderFeeds {}

struct Feed {
    hub: Arc<SignalHub>,
    player: WeakEntity<Player>,
}

/// Register a window's hub and player, once as it opens. Also prunes closed
/// windows. Prune on window liveness, not the player's: a closed popout's
/// player is its parent's and stays alive.
pub fn note_window(window: &Window, state: &AppState, cx: &mut App) {
    let id = window.window_handle().window_id();
    let live: HashSet<WindowId> = cx.windows().iter().map(|h| h.window_id()).collect();
    let feeds = cx.default_global::<ShaderFeeds>();
    feeds
        .0
        .retain(|window, feed| live.contains(window) && feed.player.upgrade().is_some());
    COVERS
        .write()
        .unwrap()
        .retain(|window, _| live.iter().any(|id| id.as_u64() == *window));
    feeds.0.insert(
        id,
        Feed {
            hub: state.signals.clone(),
            player: state.player.downgrade(),
        },
    );
}

/// The playing track's cover, per window: two windows on different tracks
/// sharing one slot would re-register each other's programs every frame.
struct CoverFeed {
    /// The path is the right identity here: cue tracks of one image share
    /// their art.
    path: Option<PathBuf>,
    /// Hashed into the program keys, so only cover-binding programs
    /// re-register on a track change.
    rev: u64,
    image: Option<Arc<AssetImage>>,
}

static COVERS: LazyLock<RwLock<HashMap<u64, CoverFeed>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// The registered cover's long-edge cap. Registered textures live until
/// the window closes (the patch never evicts), so each album costs a
/// megabyte here instead of sixteen.
const COVER_EDGE: u32 = 512;

/// Point a window's cover feed at the playing file, decoding its art when
/// the path turns over. Returns the feed's revision for the program key.
pub fn note_cover(window: u64, path: Option<&Path>) -> u64 {
    {
        let covers = COVERS.read().unwrap();
        if let Some(feed) = covers.get(&window) {
            if feed.path.as_deref() == path {
                return feed.rev;
            }
        } else if path.is_none() {
            // The fresh-window steady state; don't grow the map for it.
            return 0;
        }
    }
    // Decode outside the lock. Paint is main-thread, so nobody races the
    // write below.
    let image = path.and_then(load_cover).map(Arc::new);
    let mut covers = COVERS.write().unwrap();
    let feed = covers.entry(window).or_insert(CoverFeed {
        path: None,
        rev: 0,
        image: None,
    });
    if feed.path.as_deref() != path {
        feed.path = path.map(Path::to_path_buf);
        feed.image = image;
        feed.rev += 1;
    }
    feed.rev
}

/// [`note_cover`] fed from the window's own player. A window with no feed
/// registered is left alone; [`adopt_cover`] fills those.
pub fn poll_cover(window: &Window, cx: &App) -> u64 {
    let id = window.window_handle().window_id().as_u64();
    let Some((_, player)) = window_feed(window, cx) else {
        return COVERS.read().unwrap().get(&id).map_or(0, |feed| feed.rev);
    };
    let path = player
        .read(cx)
        .now_playing()
        .and_then(|now| now.path().map(|path| path.to_path_buf()));
    note_cover(id, path.as_deref())
}

/// For the child-window sweep: a child runs the primary's program, so it
/// takes its art too.
pub fn adopt_cover(from: u64, to: u64) {
    let mut covers = COVERS.write().unwrap();
    let Some(source) = covers
        .get(&from)
        .map(|feed| (feed.path.clone(), feed.image.clone()))
    else {
        return;
    };
    let feed = covers.entry(to).or_insert(CoverFeed {
        path: None,
        rev: 0,
        image: None,
    });
    if feed.path != source.0 {
        (feed.path, feed.image) = source;
        feed.rev += 1;
    }
}

pub(crate) fn window_cover(window: u64) -> Option<Arc<AssetImage>> {
    COVERS.read().unwrap().get(&window)?.image.clone()
}

fn load_cover(path: &Path) -> Option<AssetImage> {
    let (bytes, _mime) = rox_library::art::cover_art_of(path, rox_library::art::ArtKind::Front)?;
    let image = image::load_from_memory(&bytes).ok()?;
    let image = if image.width().max(image.height()) > COVER_EDGE {
        image.resize(
            COVER_EDGE,
            COVER_EDGE,
            image::imageops::FilterType::Triangle,
        )
    } else {
        image
    };
    let image = image.to_rgba8();
    Some(AssetImage {
        width: image.width(),
        height: image.height(),
        rgba8: image.into_raw(),
    })
}

/// What gpui returns on a renderer with no shader pipeline: a Mac build
/// without `macos-blade`, or a DirectX device below shader model 5.0.
const NO_PIPELINE: &str = "unsupported";

/// Whether a registration failure is the backend rather than the shader.
/// The asset step fails first when a program declares an image, so the
/// word can arrive prefixed.
pub fn unsupported(error: &str) -> bool {
    error == NO_PIPELINE || error.ends_with(&format!(": {NO_PIPELINE}"))
}

/// Shown instead of a bare "unsupported", which sends people hunting
/// through WGSL that's fine.
pub const NO_PIPELINE_NOTE: &str = "This build renders through a backend with no shader pipeline. Shaders ride \
     blade's render pipelines, so every source gets turned down whatever it says.";

/// No full stop: it's a banner headline first.
pub const NO_PIPELINE_TITLE: &str = "Shaders don't run on this build";

/// The last compile message per panel, for its settings window's readout.
static ERRORS: LazyLock<RwLock<HashMap<EntityId, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn error(panel: EntityId) -> Option<String> {
    ERRORS.read().unwrap().get(&panel).cloned()
}

pub fn note_error(panel: EntityId, message: Option<String>) {
    let mut errors = ERRORS.write().unwrap();
    match message {
        Some(message) => {
            errors.insert(panel, message);
        }
        None => {
            errors.remove(&panel);
        }
    }
}

/// Failed registrations by program key. gpui caches successes but re-runs
/// naga on every rejection, and the wrapper registers from paint.
static FAILED: LazyLock<RwLock<HashMap<(u64, u64), String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

fn source_hash(source: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

/// The pool generation is in the key because a program can fail over an
/// image, and fixing one changes no source text. Without it the error
/// could only be cleared by editing the shader. `cover` is zero for
/// programs that don't bind [`COVER_SOURCE`].
fn program_key(window: u64, source: &str, ctx: &ProgramCtx, cover: u64) -> (u64, u64) {
    use std::hash::{Hash as _, Hasher as _};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    ctx.name.hash(&mut hasher);
    ctx.path.hash(&mut hasher);
    rox_core::settings::shader_pool_rev().hash(&mut hasher);
    cover.hash(&mut hasher);
    (window, hasher.finish())
}

/// What a panel's surface is running, kept out here because the wrapper
/// paints with only the entity id. Keyed by window too: a popped-out panel
/// draws in two, and a `UserShaderId` belongs to the window that made it.
struct Live {
    /// The config source this entry was armed for. A settings edit moves
    /// it and re-arms the watch, so the file can't pull old text back.
    config: u64,
    watch: SourceWatch,
    /// The file's text, once a reload has moved past the config's copy.
    hot: Option<String>,
    /// The last clean registration and its program key. Keeps painting
    /// while an edit is broken, and short-circuits registration while the
    /// key holds.
    good: Option<(u64, UserShaderId)>,
    touched: Instant,
}

static LIVE: LazyLock<RwLock<HashMap<(u64, EntityId), Live>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// What each publishing panel drew in its body rect, for `meta[7]`, so a
/// frame shader can hug a letterboxed picture.
static CONTENT: LazyLock<RwLock<HashMap<EntityId, f32>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Width over height for content letterboxed centered in the rect,
/// negative for content filling it, zero for no claim. Cheap per render.
pub fn note_content_shape(panel: EntityId, shape: f32) {
    CONTENT.write().unwrap().insert(panel, shape);
}

/// The publisher's job on release.
pub fn forget_content_shape(panel: EntityId) {
    CONTENT.write().unwrap().remove(&panel);
}

fn content_shape(panel: EntityId) -> f32 {
    CONTENT.read().unwrap().get(&panel).copied().unwrap_or(0.0)
}

/// Long enough that a panel in a background window keeps its state.
const LIVE_TTL: Duration = Duration::from_secs(300);

/// The source a panel is running when a hot reload has moved past its
/// config. The settings window folds this back into the config.
pub fn hot_source(panel: EntityId) -> Option<String> {
    LIVE.read()
        .unwrap()
        .iter()
        .find(|((_, id), live)| *id == panel && live.hot.is_some())
        .and_then(|(_, live)| live.hot.clone())
}

/// The render side of a panel's shader, built fresh each render from the
/// chrome and held by the [`Themed`](super::themed) wrapper.
pub struct PanelSurface {
    source: String,
    name: Option<String>,
    path: Option<PathBuf>,
    routes: Vec<Route>,
    manual: Vec<(u8, f32)>,
    run_when_idle: bool,
    /// The chrome margin, so the shader leaves the backdrop gutter alone.
    inset: Sides,
    /// Read at build, since the wrapper brackets the mask span before
    /// registration has reported anything.
    wants_mask: bool,
}

impl PanelSurface {
    /// None when there's no runnable shader, including one waiting on
    /// approval: the panel renders as if the shader were off.
    ///
    /// Pool names resolve here, once per render and off the frame loop, so
    /// the pool's copy goes through the same approval gate as inline text.
    pub fn build(chrome: &PanelChrome, margin: Sides) -> Option<PanelSurface> {
        let shader = chrome.shader.as_ref().filter(|s| s.runnable())?;
        let source = resolve_source(shader.name.as_deref(), &shader.source)?;
        if !approved(&source) {
            return None;
        }
        let wants_mask = uses_mask(&source);
        Some(PanelSurface {
            wants_mask,
            source,
            name: shader.name.clone(),
            // Never watch the panel's bookmark on a named surface: it points
            // at pre-name inline text and would pull it over the pool's
            // source. The pool entry keeps its own bookmark.
            path: shader.name.is_none().then(|| shader.path.clone()).flatten(),
            routes: shader.routes.clone(),
            manual: shader.manual.clone(),
            run_when_idle: shader.run_when_idle,
            inset: margin,
        })
    }

    pub fn wants_mask(&self) -> bool {
        self.wants_mask
    }

    /// Record the shader over the panel's body, after the body has painted.
    /// A source that won't compile keeps the last good one on screen.
    pub fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let panel = window.current_view();
        let window_id = window.window_handle().window_id().as_u64();
        let (source, last_good) = self.current(window_id, panel);
        let ctx = ProgramCtx::of(self.name.as_deref(), self.path.as_deref());
        let cover = if uses_cover(&source) {
            poll_cover(window, cx)
        } else {
            0
        };
        let key = program_key(window_id, &source, &ctx, cover);
        let shader = match last_good {
            Some((seen, shader)) if seen == key.1 => Some(shader),
            _ => {
                let last_good = last_good.map(|(_, shader)| shader);
                match FAILED.read().unwrap().get(&key).cloned() {
                    Some(message) => {
                        note_error(panel, Some(message));
                        last_good
                    }
                    None => match register_program(window, &source, &ctx) {
                        Ok(shader) => {
                            note_error(panel, None);
                            self.note_good(window_id, panel, key.1, shader);
                            Some(shader)
                        }
                        Err(message) => {
                            FAILED.write().unwrap().insert(key, message.clone());
                            note_error(panel, Some(message));
                            last_good
                        }
                    },
                }
            }
        };
        let Some(shader) = shader else {
            return;
        };
        let (signals, live) = self.signals(window, cx);
        let mut meta = meta_slots(window, cx);
        // Only panel surfaces fill this; the Shader panel and backdrop read
        // zero.
        meta[7] = content_shape(panel);
        // A cursor reader keeps its own frames coming until presence fades
        // out, and the watch wakes it when the hand returns.
        let cursor = reads_cursor(&source);
        if cursor {
            watch_cursor(window);
        }
        let bounds = body_rect(bounds, self.inset);
        // Caps decide the path: screen, last frame, images or multiple
        // passes need the region pass; uniforms-only is an in-scene quad.
        // Getting this backwards paints nothing.
        let screen = window
            .user_shader_caps(shader)
            .is_some_and(|caps| caps.screen_pass_only());
        if screen {
            window.paint_screen_shader(bounds, shader, panel.as_u64(), signals, meta);
        } else {
            window.paint_user_shader(bounds, shader, signals, meta);
        }
        // Docked panels render cached, so an animating shader must dirty its
        // panel every frame. Use `request_animation_frame`, which notifies
        // only this view; a window `refresh` rebuilds every view uncached
        // and stalls the frame loop.
        if live || self.run_when_idle || (cursor && meta[6] > 0.0) {
            window.request_animation_frame();
        }
    }

    /// The source to run this frame and the last one that compiled, with
    /// hot reload applied.
    ///
    /// Only reached by an approved surface. A pending source never gets
    /// here, so a bundle can't have rox read a path of its choosing and
    /// trust what comes back.
    fn current(&self, window: u64, panel: EntityId) -> (String, Option<(u64, UserShaderId)>) {
        // Throttled and app-wide, so the first surface in a frame pays and
        // the rest cost an elapsed check.
        poll_pool();
        let config = source_hash(&self.source);
        let mut fresh = None;
        let (source, good) = {
            let mut live = LIVE.write().unwrap();
            let entry = match live.entry((window, panel)) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    // Unseeded: a restored snapshot may not match its file.
                    entry.insert(Live {
                        config,
                        watch: SourceWatch::default(),
                        hot: None,
                        good: None,
                        touched: Instant::now(),
                    })
                }
            };
            if entry.config != config {
                // A settings edit wins over the file; re-arm from it.
                entry.config = config;
                entry.hot = None;
                entry.watch = SourceWatch::seeded(self.path.as_deref());
            }
            entry.touched = Instant::now();
            if let Some(path) = &self.path
                && let Some(text) = entry.watch.poll(path)
            {
                let running = entry.hot.as_deref().unwrap_or(&self.source);
                if text.trim() != running.trim() {
                    // The user pointed rox at this file, so approve what
                    // comes out of it, or the gate trips on restart.
                    fresh = Some(text.clone());
                    entry.hot = Some(text);
                }
            }
            (
                entry.hot.clone().unwrap_or_else(|| self.source.clone()),
                entry.good,
            )
        };
        // Outside the lock: approving writes the settings file.
        if let Some(text) = fresh {
            approve(&text);
        }
        (source, good)
    }

    /// Remember a clean registration, and sweep stale entries.
    fn note_good(&self, window: u64, panel: EntityId, key: u64, shader: UserShaderId) {
        let mut live = LIVE.write().unwrap();
        if let Some(entry) = live.get_mut(&(window, panel)) {
            entry.good = Some((key, shader));
        }
        if live.len() > 32 {
            cursor::sweep_cursor();
            let now = Instant::now();
            live.retain(|_, entry| now.duration_since(entry.touched) < LIVE_TTL);
        }
    }

    /// This frame's slot values, and whether the hub is moving, settling
    /// included, or a parked surface would hold a fade halfway down. This
    /// ticks the hub, since a panel shader can be the only audio watcher;
    /// the hub dedupes ticks.
    fn signals(&self, window: &Window, cx: &App) -> ([f32; SLOTS], bool) {
        let mut targets = SlotTargets::default();
        seed_manual(&mut targets, &self.manual);
        let Some((hub, _)) = window_feed(window, cx) else {
            return (targets.slots, false);
        };
        signal_ui::apply_routes(&self.routes, &hub, &mut targets);
        (targets.slots, hub.live() || hub.settling())
    }
}

/// The bounds pulled in by the chrome margin. Each inset stops at half its
/// axis, so an oversized margin closes the rect instead of inverting it.
fn body_rect(bounds: Bounds<Pixels>, inset: Sides) -> Bounds<Pixels> {
    let hold = |value: f32, axis: Pixels| gpui::px(value.max(0.0)).min(axis / 2.0);
    let (width, height) = (bounds.size.width, bounds.size.height);
    let (top, bottom) = (hold(inset.top, height), hold(inset.bottom, height));
    let (left, right) = (hold(inset.left, width), hold(inset.right, width));
    Bounds {
        origin: bounds.origin + gpui::point(left, top),
        size: gpui::size(width - left - right, height - top - bottom),
    }
}

/// The player behind a window, off the [`note_window`] registry. Public
/// for the backdrop layers, which paint from a hook with only a window.
pub fn window_player(window: &Window, cx: &App) -> Option<gpui::Entity<Player>> {
    window_feed(window, cx).map(|(_, player)| player)
}

fn window_feed(window: &Window, cx: &App) -> Option<(Arc<SignalHub>, gpui::Entity<Player>)> {
    let feeds = cx.try_global::<ShaderFeeds>()?;
    let feed = feeds.0.get(&window.window_handle().window_id())?;
    Some((feed.hub.clone(), feed.player.upgrade()?))
}

/// The eight `meta` floats every rox shader can count on. Slot 7, the
/// content shape, reads zero here; a panel surface fills it (see
/// [`note_content_shape`]).
pub fn meta_slots(window: &Window, cx: &App) -> [f32; 8] {
    let mut meta = [0.0f32; 8];
    // Cursor and theme slots go before the feed check: a window with no
    // player registered still has both.
    meta[6] = cursor_presence(window);
    // Root background luma, 0 black to 1 white.
    let bg = rox_design::palette::bg_root_opaque();
    meta[4] = (0.2126 * bg.r + 0.7152 * bg.g + 0.0722 * bg.b).clamp(0.0, 1.0);
    // The theme pick, 1 light. Not derivable from slot 4: song theming can
    // swap the rendered side and moves luma within a side.
    meta[5] = match rox_design::palette::mode() {
        rox_design::palette::Mode::Light => 1.0,
        rox_design::palette::Mode::Dark => 0.0,
    };
    let Some((_, player)) = window_feed(window, cx) else {
        return meta;
    };
    let player = player.read(cx);
    // Volume runs to 200%, but the slot is documented 0..1.
    meta[0] = if player.muted() {
        0.0
    } else {
        player.volume().clamp(0.0, 1.0)
    };
    if let Some(now) = player.now_playing() {
        let duration = now.duration_secs.unwrap_or(0.0);
        if duration > 0.0 {
            meta[1] = (now.position_secs / duration).clamp(0.0, 1.0) as f32;
        }
        meta[3] = duration as f32;
    }
    meta[2] = if player.is_playing() { 1.0 } else { 0.0 };
    meta
}

/// The shader pool is app-global. Any test here or in [`chain`] that
/// swaps it takes this first.
#[cfg(test)]
static POOL_GUARD: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use rox_viz::AudioFeed;
    use rox_viz::signal::Source;

    #[test]
    fn slot_targets_round_trip() {
        for slot in 0..SLOTS {
            assert_eq!(target_slot(&slot_target(slot)), Some(slot));
        }
        assert_eq!(target_slot("slot16"), None);
        assert_eq!(target_slot("bass"), None);
        assert_eq!(target_slot(""), None);
    }

    #[test]
    fn a_config_lands_on_the_picker_entry_it_came_from() {
        assert_eq!(pick(None, None, None), Pick::Empty);
        assert_eq!(pick(None, None, Some("  \n ")), Pick::Empty);
        assert_eq!(pick_label(&Pick::Empty), rox_i18n::t!("shader-pick-none"));

        assert_eq!(pick(None, None, Some(PLASMA)), Pick::Example(0));
        assert_eq!(pick(None, None, Some(TRAILS)), Pick::Example(1));
        assert_eq!(
            pick(None, None, Some(&format!("{PLASMA}\n\n"))),
            Pick::Example(0)
        );
        assert_eq!(pick_label(&Pick::Example(0)), "Plasma");
        assert_eq!(pick_label(&Pick::Example(1)), "Trails");

        // A file beats the example match.
        let path = PathBuf::from("/home/someone/shaders/grain.wgsl");
        assert_eq!(pick(None, Some(&path), Some(PLASMA)), Pick::File(path));
        assert_eq!(
            pick_label(&Pick::File("/home/someone/shaders/grain.wgsl".into())),
            "grain"
        );
        assert_eq!(pick_label(&Pick::File("/".into())), "/");

        assert_eq!(pick(None, None, Some("// mine")), Pick::Custom);
        assert_eq!(
            pick_label(&Pick::Custom),
            rox_i18n::t!("shader-pick-custom")
        );

        // A name wins over everything inline.
        assert_eq!(
            pick(
                Some("Grain"),
                Some(&PathBuf::from("/tmp/x.wgsl")),
                Some("// mine")
            ),
            Pick::Named {
                name: "Grain".to_string(),
                missing: false,
            }
        );
        assert_eq!(
            pick(Some("Grain"), None, None),
            Pick::Named {
                name: "Grain".to_string(),
                missing: true,
            }
        );
        assert_eq!(
            pick_label(&Pick::Named {
                name: "Grain".to_string(),
                missing: false,
            }),
            "Grain"
        );
        assert_eq!(
            pick_label(&Pick::Named {
                name: "Grain".to_string(),
                missing: true,
            }),
            "Grain (missing)"
        );
    }

    #[test]
    fn the_cover_feed_revs_only_when_the_track_turns_over() {
        // Ids no real window takes, since the feed map is app-global.
        let window = u64::MAX - 7;
        let child = u64::MAX - 8;
        assert_eq!(
            note_cover(window, None),
            0,
            "idle from the start writes nothing"
        );
        let a = PathBuf::from("/nowhere/a.flac");
        assert_eq!(note_cover(window, Some(&a)), 1);
        assert_eq!(note_cover(window, Some(&a)), 1, "same track holds still");
        assert_eq!(
            note_cover(window, Some(&PathBuf::from("/nowhere/b.flac"))),
            2
        );
        assert_eq!(note_cover(window, None), 3, "stopping is a change too");
        assert!(window_cover(window).is_none());

        note_cover(window, Some(&a));
        adopt_cover(window, child);
        let rev = |id: u64| COVERS.read().unwrap().get(&id).map(|feed| feed.rev);
        assert_eq!(rev(child), Some(1));
        adopt_cover(window, child);
        assert_eq!(rev(child), Some(1), "adopting the same art moves nothing");
    }

    #[test]
    fn every_example_brings_its_own_blurb() {
        for (index, preset) in PRESETS.iter().enumerate() {
            // An unwritten message renders as the missing marker.
            let blurb = pick_blurb(index);
            assert!(
                !blurb.trim().is_empty() && !blurb.contains('⟦'),
                "{} ships without a line to print under it",
                preset.label
            );
        }
        assert_eq!(pick_blurb(PRESETS.len()), "");
    }

    #[test]
    fn resolve_source_reads_the_pool_before_the_inline_copy() {
        let _pool = POOL_GUARD.lock().unwrap_or_else(|held| held.into_inner());
        rox_core::settings::note_shader_pool(vec![rox_core::settings::NamedShader {
            name: "Grain".to_string(),
            source: "// the pool's grain".to_string(),
            path: None,
            assets: Vec::new(),
        }]);

        assert_eq!(
            resolve_source(Some("Grain"), "// the panel's own"),
            Some("// the pool's grain".to_string())
        );
        assert_eq!(resolve_source(Some("Bloom"), "// the panel's own"), None);
        assert_eq!(
            resolve_source(None, "// the panel's own"),
            Some("// the panel's own".to_string())
        );
        assert_eq!(resolve_source(None, "   \n "), None);
        assert_eq!(resolve_source(None, ""), None);

        rox_core::settings::note_shader_pool(Vec::new());
        assert_eq!(resolve_source(Some("Grain"), "// the panel's own"), None);
    }

    #[test]
    fn a_named_shader_is_runnable_without_its_own_source() {
        let named = PanelShader {
            name: Some("Grain".to_string()),
            source: String::new(),
            ..PanelShader::default()
        };
        assert!(named.runnable());

        let off = PanelShader {
            enabled: false,
            ..named.clone()
        };
        assert!(!off.runnable(), "the switch still wins");

        let bare = PanelShader {
            source: "  \n".to_string(),
            ..PanelShader::default()
        };
        assert!(!bare.runnable(), "no name and no text is nothing to run");
    }

    #[test]
    fn a_pool_name_rides_the_shader_config() {
        let shader = PanelShader {
            name: Some("Grain".to_string()),
            ..PanelShader::default()
        };
        let dumped = serde_json::to_value(&shader).expect("dump");
        assert_eq!(dumped["name"], "Grain");
        let read: PanelShader = serde_json::from_value(dumped).expect("read back");
        assert_eq!(read.name.as_deref(), Some("Grain"));

        let nameless = serde_json::to_value(PanelShader::default()).expect("dump");
        assert!(
            nameless.get("name").is_none(),
            "an unnamed shader writes no key: {nameless}"
        );

        let older: PanelShader = serde_json::from_value(serde_json::json!({
            "enabled": true,
            "source": "// mine",
        }))
        .expect("older dumps still load");
        assert!(older.name.is_none());
        assert_eq!(older.source, "// mine");
    }

    #[test]
    fn slot_labels_read_the_comment_convention() {
        let source = "// @slot 0: bass\n\
                      //@slot 3 : the  drums \n\
                      // @slot 99: out of range\n\
                      // @slot two: not a number\n\
                      // just a comment\n\
                      fn fs_user(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }";
        let labels = slot_labels(source);
        assert_eq!(labels[0].as_deref(), Some("bass"));
        assert_eq!(labels[3].as_deref(), Some("the  drums"));
        assert_eq!(labels[1], None);
        assert_eq!(slot_label(&labels, 0), "bass");
        assert_eq!(slot_label(&labels, 7), "slot 7");
    }

    /// A hub with one band signal driven to full by a real tone. The tick
    /// throttles, so the attack takes wall clock.
    fn loud_hub() -> (SignalHub, u64) {
        let feed = Arc::new(AudioFeed::new());
        let hub = SignalHub::with_feed(Vec::new(), feed.clone());
        let (id, _) = hub.add(
            Source::Band {
                lo: 800.0,
                hi: 2000.0,
            },
            0.0,
        );
        let mut phase = 0.0f32;
        for _ in 0..60 {
            let mut samples = vec![0.0f32; 4096];
            for frame in samples.chunks_mut(2) {
                phase += std::f32::consts::TAU * 1170.0 / 48_000.0;
                frame[0] = phase.sin();
                frame[1] = frame[0];
            }
            feed.push(&samples);
            // The read advances the engine.
            hub.value(id);
            std::thread::sleep(std::time::Duration::from_millis(4));
        }
        (hub, id)
    }

    #[test]
    fn routes_resolve_into_slots() {
        let (hub, loud) = loud_hub();
        assert!(
            hub.value(loud).unwrap_or(0.0) > 0.5,
            "the band signal should be up before the routes are read"
        );

        let route = |signal, target: String, from, to, enabled| Route {
            enabled,
            signal,
            target,
            from,
            to,
        };
        let routes = vec![
            route(loud, slot_target(2), 0.0, 1.0, true),
            route(loud, slot_target(5), 0.0, 0.5, true),
            route(loud, slot_target(7), 0.0, 1.0, false),
            route(999, slot_target(9), 0.0, 1.0, true),
            route(loud, "nowhere".to_string(), 0.0, 1.0, true),
            route(loud, slot_target(SLOTS), 0.0, 1.0, true),
        ];
        let mut targets = SlotTargets::default();
        signal_ui::apply_routes(&routes, &hub, &mut targets);

        let full = targets.slots[2];
        assert!(full > 0.5, "slot 2 should carry the signal, got {full}");
        assert!(
            (targets.slots[5] - full * 0.5).abs() < 0.05,
            "slot 5 should sit at half the span"
        );
        assert_eq!(targets.slots[7], 0.0);
        assert_eq!(targets.slots[9], 0.0);
        assert_eq!(targets.slots[0], 0.0);
    }

    #[test]
    fn targets_list_every_slot_by_name() {
        let targets = SlotTargets::labelled("// @slot 1: mids\n");
        let listed = targets.targets();
        assert_eq!(listed.len(), SLOTS);
        assert_eq!(listed[1], ("slot1".to_string(), "mids".to_string()));
        assert_eq!(listed[4], ("slot4".to_string(), "slot 4".to_string()));
    }

    /// Unique per call, so parallel tests can't see each other's approvals.
    fn novel_source(tag: &str) -> String {
        format!(
            "// {tag} {:?}\nfn fs_user(uv: vec2<f32>) -> vec4<f32> {{ return vec4<f32>(uv, 0.0, 1.0); }}",
            std::time::SystemTime::now()
        )
    }

    #[test]
    fn a_source_that_arrives_serialized_waits() {
        let source = novel_source("arrived");
        assert!(
            !approved(&source),
            "a source nobody has agreed to must not run"
        );
        // The Approve button minus the settings write, which would land in
        // the machine's real session file.
        let print = fingerprint(&source);
        assert!(rox_core::settings::note_approved(&print));
        assert!(approved(&source), "an approved hash runs");
        assert!(!approved(&novel_source("arrived twice")));
        rox_core::settings::forget_approved(&print);
        assert!(!approved(&source), "and the gate closes again");
    }

    #[test]
    fn the_builtins_need_no_list() {
        for Preset { label, source, .. } in PRESETS {
            assert!(builtin(source), "{label} is one of ours");
            assert!(approved(source), "{label} ships with the binary");
            assert!(
                !rox_core::settings::shader_approved(&fingerprint(source)),
                "{label} shouldn't need a list entry to pass the gate"
            );
        }
        approve(PLASMA);
        assert!(!rox_core::settings::shader_approved(&fingerprint(PLASMA)));
        assert!(approved(""));
        assert!(approved("   \n "));
    }

    #[test]
    fn fingerprints_ignore_the_edges_and_nothing_else() {
        let source = "fn fs_user(uv: vec2<f32>) -> vec4<f32> { return vec4<f32>(1.0); }";
        assert_eq!(fingerprint(source), fingerprint(&format!("\n{source}\n\n")));
        assert_ne!(
            fingerprint(source),
            fingerprint(&source.replace("1.0", "0.0")),
            "a changed constant is a changed shader"
        );
        let print = fingerprint(source);
        assert_eq!(print.len(), 64);
        assert!(print.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn a_builtin_survives_a_round_trip_through_a_layout() {
        // Serde's string round trip is where a trailing newline would go
        // missing.
        let dumped = serde_json::to_string(&PLASMA.to_string()).expect("dump");
        let read: String = serde_json::from_str(&dumped).expect("read");
        assert!(approved(&read));
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rox-shader-watch-{name}"));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir.join("shader.wgsl")
    }

    #[test]
    fn an_unseeded_watch_reads_once_then_waits_for_the_file_to_move() {
        let path = scratch("unseeded");
        std::fs::write(&path, "one").expect("write");
        let mut watch = SourceWatch::default();
        assert_eq!(watch.poll(&path).as_deref(), Some("one"));
        // Throttled.
        assert_eq!(watch.poll(&path), None);
        watch.checked = None;
        assert_eq!(watch.poll(&path), None, "nothing moved");
        // mtime only resolves to the second, so the change is a length.
        watch.checked = None;
        std::fs::write(&path, "one two three").expect("rewrite");
        assert_eq!(watch.poll(&path).as_deref(), Some("one two three"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_seeded_watch_waits_for_the_next_edit() {
        let path = scratch("seeded");
        std::fs::write(&path, "one").expect("write");
        let mut watch = SourceWatch::seeded(Some(path.as_path()));
        watch.checked = None;
        assert_eq!(watch.poll(&path), None);
        watch.checked = None;
        std::fs::write(&path, "one two three").expect("rewrite");
        assert_eq!(watch.poll(&path).as_deref(), Some("one two three"));
        watch.checked = None;
        std::fs::remove_file(&path).ok();
        assert_eq!(watch.poll(&path), None);
        watch.checked = None;
        std::fs::write(&path, "back again, longer").expect("rewrite");
        assert_eq!(watch.poll(&path).as_deref(), Some("back again, longer"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_watch_with_no_file_has_nothing_to_seed() {
        let watch = SourceWatch::seeded(None);
        assert!(!watch.seeded);
        assert!(watch.stamp.is_none());
    }

    fn eject_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("rox-shader-eject-{name}"));
        std::fs::remove_dir_all(&root).ok();
        root
    }

    #[test]
    fn ejecting_over_a_diverged_file_takes_the_next_name() {
        let root = eject_root("collision");
        let named = |stem: &str| rox_core::settings::shader_eject_path_in(&root, "Nightfall", stem);

        let first = eject_in(&root, "Nightfall", "Grain", "// one", &[]).expect("eject");
        assert_eq!(first, named("Grain"));
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "// one");

        let again = eject_in(&root, "Nightfall", "Grain", "\n// one\n", &[]).expect("eject");
        assert_eq!(again, first);

        let second = eject_in(&root, "Nightfall", "Grain", "// two", &[]).expect("eject");
        assert_eq!(second, named("Grain-2"));
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "\n// one\n");

        let third = eject_in(&root, "Nightfall", "Grain", "// three", &[]).expect("eject");
        assert_eq!(third, named("Grain-3"));
        assert_eq!(
            eject_in(&root, "Nightfall", "Grain", "// two", &[]).expect("eject"),
            named("Grain-2")
        );

        let awkward = eject_in(&root, "Nightfall", "a/b", "// slashed", &[]).expect("eject");
        assert_eq!(awkward, named("a b"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_pool_watch_pulls_an_edit_into_its_entry() {
        let dir = eject_root("pool-watch");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("grain.wgsl");
        std::fs::write(&path, "// one").expect("write");

        let entry =
            |name: &str, source: &str, path: Option<PathBuf>| rox_core::settings::NamedShader {
                name: name.to_string(),
                source: source.to_string(),
                path,
                assets: Vec::new(),
            };
        let mut pool = vec![
            entry("Grain", "// one", Some(path.clone())),
            entry("Bloom", "// bloom", None),
        ];
        let mut stamps = HashMap::new();

        assert!(!pool_reload(&mut stamps, &mut pool).changed);

        std::fs::write(&path, "// one, and then some more").expect("rewrite");
        assert_eq!(
            pool_reload(&mut stamps, &mut pool).fresh,
            vec!["// one, and then some more".to_string()]
        );
        assert_eq!(pool[0].source, "// one, and then some more");
        assert!(!pool_reload(&mut stamps, &mut pool).changed);
        assert_eq!(pool[1].source, "// bloom");

        assert!(!pool_reload(&mut stamps, &mut Vec::new()).changed);
        assert!(stamps.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    fn plate(red: u8) -> Vec<u8> {
        let mut image = image::RgbaImage::new(1, 1);
        image.put_pixel(0, 0, image::Rgba([red, 0, 0, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("encode");
        bytes.into_inner()
    }

    #[test]
    fn ejecting_writes_the_images_beside_the_shader() {
        let root = eject_root("assets");
        let source = "// @asset plate: plate.png\nfn fs_user() {}";
        let assets = vec![rox_core::settings::ShaderAsset::from_bytes(
            "plate.png",
            &plate(200),
        )];
        let path = eject_in(&root, "Nightfall", "Stamp", source, &assets).expect("eject");
        let beside = path.parent().unwrap().join("plate.png");
        assert_eq!(std::fs::read(&beside).unwrap(), plate(200));

        // Images overwrite rather than sliding down to a variant.
        std::fs::write(&beside, b"scribbled on").expect("write");
        let again = eject_in(&root, "Nightfall", "Stamp", source, &assets).expect("eject");
        assert_eq!(again, path);
        assert_eq!(std::fs::read(&beside).unwrap(), plate(200));

        let broken = vec![rox_core::settings::ShaderAsset {
            file: "plate.png".to_string(),
            data: "not base64 at all!!".to_string(),
        }];
        assert!(eject_in(&root, "Nightfall", "Stamp", source, &broken).is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_pool_watch_pulls_an_image_edit_into_its_entry() {
        let dir = eject_root("pool-assets");
        std::fs::create_dir_all(&dir).expect("scratch dir");
        let path = dir.join("stamp.wgsl");
        let source = "// @asset plate: plate.png\nfn fs_user() {}";
        std::fs::write(&path, source).expect("write");
        std::fs::write(dir.join("plate.png"), plate(10)).expect("write");

        let mut pool = vec![rox_core::settings::NamedShader {
            name: "Stamp".to_string(),
            source: source.to_string(),
            path: Some(path.clone()),
            assets: Vec::new(),
        }];
        let mut stamps = HashMap::new();

        let edits = pool_reload(&mut stamps, &mut pool);
        assert!(edits.changed, "a new image is news");
        assert!(edits.fresh.is_empty(), "and it approves nothing: it's data");
        assert_eq!(pool[0].assets.len(), 1);
        assert_eq!(pool[0].assets[0].file, "plate.png");
        assert_eq!(pool[0].assets[0].decode().unwrap(), plate(10));

        assert!(!pool_reload(&mut stamps, &mut pool).changed);

        // mtime only resolves to the second, so the change is a length.
        std::fs::write(dir.join("plate.png"), [plate(10), plate(240)].concat()).expect("rewrite");
        assert!(pool_reload(&mut stamps, &mut pool).changed);
        assert_eq!(
            pool[0].assets[0].decode().unwrap(),
            [plate(10), plate(240)].concat()
        );

        std::fs::write(&path, "fn fs_user() {}").expect("rewrite");
        let edits = pool_reload(&mut stamps, &mut pool);
        assert_eq!(edits.fresh, vec!["fn fs_user() {}".to_string()]);
        assert_eq!(pool[0].assets.len(), 1, "the bytes stay with the entry");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_ejected_name_falls_back_to_the_source() {
        assert_eq!(eject_name("  Wall  ", "// mine"), "Wall");
        let hashed = eject_name("", "// mine");
        assert!(hashed.starts_with("shader-"), "{hashed}");
        assert_eq!(hashed.len(), "shader-".len() + 8);
        assert_ne!(hashed, eject_name("", "// somebody else's"));
    }

    #[test]
    fn body_rect_pulls_in_by_the_margin() {
        let bounds = Bounds {
            origin: gpui::point(gpui::px(10.), gpui::px(20.)),
            size: gpui::size(gpui::px(100.), gpui::px(50.)),
        };
        let inner = body_rect(bounds, Sides::all(5.0));
        assert_eq!(inner.origin.x, gpui::px(15.));
        assert_eq!(inner.origin.y, gpui::px(25.));
        assert_eq!(inner.size.width, gpui::px(90.));
        assert_eq!(inner.size.height, gpui::px(40.));
        let lopsided = body_rect(
            bounds,
            Sides::ZERO.with(rox_design::palette::Side::Left, 8.0),
        );
        assert_eq!(lopsided.origin.x, gpui::px(18.));
        assert_eq!(lopsided.origin.y, gpui::px(20.));
        assert_eq!(lopsided.size.width, gpui::px(92.));
        assert_eq!(lopsided.size.height, gpui::px(50.));
        let squeezed = body_rect(bounds, Sides::all(400.0));
        assert!(squeezed.size.width >= gpui::px(0.));
        assert!(squeezed.size.height >= gpui::px(0.));
    }
}
