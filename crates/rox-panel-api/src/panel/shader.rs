//! Per-panel surface shaders: any panel can carry a WGSL fragment stage
//! that runs over its own body rect, layered under the app-wide post
//! shader. The config rides [`PanelChrome`](super::PanelChrome), so
//! persistence, duplication, and workspace bundles come free; the render
//! side is [`PanelSurface`], recorded by the [`Themed`](super::themed)
//! wrapper after the panel's body has painted.
//!
//! Three pieces live here because the upcoming Shader panel wants them
//! too: the slot targets a route list resolves into, the `// @slot n:`
//! label convention, and the eight `meta` floats every rox shader can
//! count on.

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

pub use chain::{
    fallback_cover, parse_chain, register_program, resolve_assets, uses_cover, AssetImage,
    AssetRef, ChainSpec, PassSpec, ProgramCtx, COVER_SOURCE,
};

/// How many signal slots a shader sees, the uniform block's width.
pub const SLOTS: usize = 16;

/// The builtins, shared by both shader surfaces: the Shader panel offers
/// them as presets, and the gate below trusts them by construction. Each one
/// demonstrates a different part of the contract, so together they double as
/// the authoring reference: Plasma is a pure primitive, Trails reads its own
/// last frame and proves the region pass, Sheen is a transparent overlay
/// meant to ride another panel's body, Cover and Badge bind the playing
/// track's art, Lamp reads the mouse, Cube fakes a third dimension from
/// uniforms alone, Bloom is a two-pass chain, and Tube samples the screen
/// under it.
pub const PLASMA: &str = include_str!("shader/plasma.wgsl");
pub const TRAILS: &str = include_str!("shader/trails.wgsl");
pub const SHEEN: &str = include_str!("shader/sheen.wgsl");
pub const COVER: &str = include_str!("shader/cover.wgsl");
pub const BADGE: &str = include_str!("shader/badge.wgsl");
pub const LAMP: &str = include_str!("shader/lamp.wgsl");
pub const CUBE: &str = include_str!("shader/cube.wgsl");
pub const BLOOM: &str = include_str!("shader/bloom.wgsl");
pub const TUBE: &str = include_str!("shader/tube.wgsl");

/// One shipped example: what to call it, the one line the settings pages
/// print under it once it's picked, and the WGSL itself.
///
/// The blurb rides the entry rather than sitting in a section note, so
/// adding an example stays a file plus a line here instead of a file, a
/// line, and a sentence somebody has to remember to edit somewhere else.
/// Whether this shader rides content or replaces it isn't a field here: it
/// rides the source as an `// @overlay` line, read by [`overlay`]. A pool
/// shader out of somebody's bundle has no Rust struct to carry a flag on,
/// and the picker groups both lists by the same read, so the answer lives
/// in the one place both kinds of shader have.
pub struct Preset {
    pub label: &'static str,
    pub blurb: &'static str,
    pub source: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        label: "Plasma",
        blurb: "Drifting colour drawn from its uniforms alone, so it costs a plain quad.",
        source: PLASMA,
    },
    Preset {
        label: "Trails",
        blurb: "Smears its own last frame, which puts it on the screen pass.",
        source: TRAILS,
    },
    Preset {
        label: "Sheen",
        blurb:
            "A vignette and a drifting gleam, transparent overlay for a panel that already draws.",
        source: SHEEN,
    },
    Preset {
        label: "Cover",
        blurb: "The playing track's art, letterboxed over a wash of its own color.",
        source: COVER,
    },
    Preset {
        label: "Badge",
        blurb: "The cover as a small card parked in a corner, with a slot to walk it around.",
        source: BADGE,
    },
    Preset {
        label: "Lamp",
        blurb: "A light that follows the cursor and answers the buttons, transparent overlay.",
        source: LAMP,
    },
    Preset {
        label: "Cube",
        blurb: "A wireframe cube tumbling in fake 3D, drawn as added light.",
        source: CUBE,
    },
    Preset {
        label: "Bloom",
        blurb: "Drifting orbs bloomed through a half-size second pass, the chain in miniature.",
        source: BLOOM,
    },
    Preset {
        label: "Tube",
        blurb: "Replays the panel under it through a curved CRT face, scanlines and all.",
        source: TUBE,
    },
];

/// How often a watched source file gets stat'd while its surface draws.
/// Twice a second: fast enough that a save in the editor lands before the
/// hand is back on the mouse, slow enough to be one syscall rather than one
/// a frame.
pub const RELOAD_EVERY: Duration = Duration::from_millis(500);

/// A panel's surface shader as it persists: the source text inline, the
/// file it was last loaded from, and the routes feeding its slots.
///
/// The source is stored inline on purpose. A workspace bundle carrying
/// only an absolute path would import as a dead shader on anyone else's
/// machine, so the path is a bookmark for the reload button and the
/// source is what actually runs.
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelShader {
    /// The switch. Off keeps the source and routes in place, unpainted.
    pub enabled: bool,
    /// The fragment stage: a `fs_user(uv)` definition, plus whatever it
    /// calls. Empty means nothing to run.
    pub source: String,
    /// A name in the workspace's shader pool. Set, the pool's copy is what
    /// runs and the inline source is ignored, which is how several panels
    /// wear one shader that the bundle's author edits in one place. See
    /// [`resolve_source`] for what a name that resolves to nothing does.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Where the source was last read from, for the reload button. None
    /// once a bundle travels to a machine that never had the file.
    pub path: Option<PathBuf>,
    /// The signal routes filling the shader's slots.
    pub routes: Vec<Route>,
    /// Hand-set slot values, from the Shader page's slot rows: what a slot
    /// reads with no route feeding it, which is how a shader's named
    /// parameters get tweaked without a signal in sight. A route on the
    /// same slot wins while it's there; the hand-set value comes back when
    /// it goes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub manual: Vec<(u8, f32)>,
    /// Keep asking for frames with the hub silent. Off, a shader over a
    /// paused player freezes where it stands and the panel costs nothing.
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
    /// Whether there is anything to paint: switched on, with either a pool
    /// name or source text of its own. A name that turns out to resolve to
    /// nothing still counts here, since whether the pool holds it is a
    /// question for [`resolve_source`] at registration and not for a config
    /// that has been asked what it wants.
    pub fn runnable(&self) -> bool {
        self.enabled && (self.name.is_some() || !self.source.trim().is_empty())
    }
}

/// The WGSL a shader config actually runs, given its optional pool name and
/// its inline source. Shared by both shader surfaces, since both grew the
/// same pair of fields.
///
/// A name wins outright: a hit hands back the pool's source, and a miss
/// hands back None so the surface paints nothing. A miss deliberately
/// doesn't fall through to the inline copy. A name says "whatever the
/// workspace calls this", and quietly running some stale text under that
/// name would be worse than a blank panel. It's the same read as a route
/// pointing at a signal that's gone, which holds its slot at zero rather
/// than picking a different signal.
///
/// Without a name, the inline source runs, and empty text is nothing to run.
///
/// Call this where source changes are already detected, which is
/// registration time, and never once a frame: it takes a lock and copies a
/// page of text. A cached answer goes stale when
/// [`shader_pool_rev`](rox_core::settings::shader_pool_rev) moves, which is
/// one atomic load to check.
pub fn resolve_source(name: Option<&str>, inline: &str) -> Option<String> {
    match name {
        Some(name) => rox_core::settings::shader_pool_get(name).map(|shader| shader.source),
        None => (!inline.trim().is_empty()).then(|| inline.to_string()),
    }
}

/// Which entry of a shader picker a config currently sits on.
///
/// Both shader surfaces grew the same handful of ways to end up with a
/// shader, and the picker's closed label, the note under it, and which
/// buttons follow all come off this one read rather than off four
/// scattered matches on the config fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    /// Nothing to run.
    Empty,
    /// One of this workspace's shaders. `missing` when the workspace
    /// doesn't hold that name anymore, which paints nothing at all.
    Named { name: String, missing: bool },
    /// A file on this machine, watched for saves.
    File(PathBuf),
    /// One of the shipped examples, by its index in [`PRESETS`].
    Example(usize),
    /// A source of its own that matches nothing else, usually one that
    /// arrived inside a layout or a bundle.
    Custom,
}

/// Work out which picker entry a config is on. `resolved` is what actually
/// runs, so a name this workspace doesn't hold arrives as None and reads as
/// missing rather than as empty.
///
/// A file bookmark beats an example match on purpose. Editing an example as
/// a file leaves the text identical for as long as it takes to make the
/// first change, and from the moment the file exists it's the thing being
/// edited, so the rows under the picker have to offer Reload rather than a
/// second eject.
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

/// What the picker's closed state reads. A file shows its stem: the whole
/// path is already spelled out in the note under the row, and a control
/// wide enough to hold one would push everything else off it.
pub fn pick_label(pick: &Pick) -> String {
    match pick {
        Pick::Empty => "None".to_string(),
        Pick::Named {
            name,
            missing: true,
        } => format!("{name} (missing)"),
        Pick::Named { name, .. } => name.clone(),
        Pick::File(path) => path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .filter(|stem| !stem.is_empty())
            .unwrap_or_else(|| path.display().to_string()),
        Pick::Example(index) => PRESETS
            .get(*index)
            .map(|preset| preset.label.to_string())
            .unwrap_or_else(|| "Custom".to_string()),
        Pick::Custom => "Custom".to_string(),
    }
}

/// The one line a picked example prints under the picker, empty for
/// anything the table doesn't hold.
pub fn pick_blurb(index: usize) -> &'static str {
    PRESETS
        .get(index)
        .map(|preset| preset.blurb)
        .unwrap_or_default()
}

/// A source's identity in the approved list: hex SHA-256 of the trimmed
/// text. Trimmed so an editor's trailing newline isn't a different program,
/// and hashed rather than stored so the list stays a few lines whatever the
/// shaders weigh.
pub fn fingerprint(source: &str) -> String {
    use sha2::{Digest as _, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(source.trim().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Whether a source is one the app ships. Builtins are approved by
/// construction: they came with the binary, so a list entry would only be a
/// second copy of a decision already made by installing rox.
pub fn builtin(source: &str) -> bool {
    let source = source.trim();
    PRESETS.iter().any(|preset| preset.source.trim() == source)
}

/// Whether this source may run on this machine.
///
/// Shaders ride layout dumps and workspace bundles as inline WGSL, so
/// applying somebody else's look hands rox somebody else's code. Nothing
/// registers until its hash is in the machine-local approved list, which
/// only a direct action writes to: a file pick, a reload, a preset, or the
/// Approve button on the panel's settings page. An empty source reads as
/// approved because there is nothing to run.
pub fn approved(source: &str) -> bool {
    source.trim().is_empty()
        || builtin(source)
        || rox_core::settings::shader_approved(&fingerprint(source))
}

/// Record a source as approved, on this machine and on disk. Every path
/// where the user themselves put the source there calls this; nothing on
/// the apply or restore side ever does.
pub fn approve(source: &str) {
    if source.trim().is_empty() || builtin(source) {
        return;
    }
    rox_core::settings::approve_shader(&fingerprint(source));
}

/// How many numbered variants an eject will try before it gives up. A
/// folder with a hundred diverged copies of one shader in it is somebody
/// who wanted a file manager, not another write.
const EJECT_VARIANTS: u32 = 99;

/// The name of the look the app is wearing, which is the folder ejected
/// shaders land in. A look that was never saved has no name, and the path
/// helper is the one that turns that into `_local`, so this hands the name
/// over exactly as it found it.
pub fn live_workspace() -> String {
    rox_core::settings::Settings::load().look.bundle.name
}

/// Write a shader out to a file an editor can open, under the live
/// workspace's folder. This is the authoring loop's front door: rox has no
/// text editor of its own, so a shader that arrived inside a bundle gets a
/// working copy here and the file watch carries the edits back.
///
/// A file already at that name is only written over when it still holds the
/// same shader, hash for hash. Anything else has diverged, whether somebody
/// edited it or another shader took the name first, and clobbering it would
/// throw away work nobody asked to lose. The eject slides down to `name-2`
/// and keeps going instead.
pub fn eject(name: &str, source: &str) -> std::io::Result<PathBuf> {
    eject_in(
        &rox_core::settings::shaders_dir(),
        &live_workspace(),
        name,
        source,
        &[],
    )
}

/// [`eject`] under a given root and with the images the shader carries,
/// which is what the tests write into rather than the folder the running
/// app ejects to.
///
/// The images land beside the `.wgsl` under their own file names, and those
/// are overwritten rather than slid down to a variant. A plate is data the
/// shader points at by name, so two shaders naming the same file mean the
/// same file, and a numbered copy would only leave the second shader
/// sampling the first one's image.
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
        // A file that reads as a different shader keeps its name; anything
        // else (the same shader, or nothing there at all) takes the write.
        // A read that fails for some other reason falls through to the write
        // below, which is where the real error comes from.
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
        format!("{name} already has {EJECT_VARIANTS} files in this workspace's shaders"),
    ))
}

/// Write a shader's images out beside its `.wgsl`. A name that decodes to
/// nothing is the entry being hand-edited into something that isn't an
/// image, which reads out here rather than at the next registration.
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

/// Eject a pool entry and bookmark the file on the entry, so [`poll_pool`]
/// watches it from then on and every panel wearing the name picks up the
/// edits. Answers the file it wrote, with the shader's images written
/// beside it.
pub fn eject_pool_entry(name: &str) -> std::io::Result<PathBuf> {
    let Some(entry) = rox_core::settings::shader_pool_get(name) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{name} isn't in this workspace's shaders"),
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

/// The file name an inline shader ejects under: what its panel is called,
/// or a short hash of the source when the panel has nothing worth using as
/// a name.
pub fn eject_name(label: &str, source: &str) -> String {
    let label = label.trim();
    if label.is_empty() {
        format!("shader-{}", &fingerprint(source)[..8])
    } else {
        label.to_string()
    }
}

/// Put an inline shader into the workspace's pool under `name`, answering
/// whether it replaced an entry that was already there. The source approves
/// on the way in, since promoting a shader is the user vouching for it as
/// much as picking its file was.
///
/// `path` is the working copy the source came from, carried over so a panel
/// that was already being edited in a file keeps its hot reload after the
/// promotion. It's the incoming shader's file either way: a replaced entry
/// drops the bookmark it had, because that file still holds the old shader
/// and leaving the two linked would have the pool watch pull it straight
/// back over what was just saved.
///
/// A shader promoted out of a file takes the images it declares with it,
/// read from beside that file. Without that the entry would travel as a
/// look with holes in it, since the plates only ever sat on this machine.
pub fn save_to_pool(name: &str, source: &str, path: Option<PathBuf>) -> bool {
    approve(source);
    let captured = sibling_assets(source, path.as_deref());
    let mut pool = rox_core::settings::shader_pool();
    let replaced = match pool.iter_mut().find(|entry| entry.name == name) {
        Some(entry) => {
            entry.source = source.to_string();
            entry.path = path;
            // Additive: what's on disk wins for the files it holds, and
            // anything the entry already carried stays, since a plate is
            // still a plate while its `@asset` line is being edited.
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

/// The images a source declares, read from the folder it came from. Empty
/// for a source with no file behind it, and for one that declares nothing,
/// so the common promotion costs a parse and no syscalls.
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

/// The pool's own hot reload, and the app's only copy of it.
///
/// A pool entry that has been ejected carries a bookmark, and every panel
/// wearing that name runs its text, so the watch belongs to the pool rather
/// than to any one of them: a per-panel watch would stat the same file once
/// per wearer and race itself writing the answer back. The lock is a
/// `try_lock` for the same reason [`SourceWatch`] throttles: this is called
/// from paint, and the second panel through in a frame has nothing to add.
static POOL_WATCH: LazyLock<Mutex<PoolWatch>> = LazyLock::new(|| Mutex::new(PoolWatch::default()));

#[derive(Default)]
struct PoolWatch {
    /// The last sweep, so the whole thing costs an elapsed check per frame
    /// rather than a stat per bookmarked entry.
    checked: Option<Instant>,
    /// Each bookmarked entry's last look at its files, by name. An entry
    /// with no stamp yet reads them once, the same rule the per-panel watch
    /// keeps: an edit made while rox was closed should land on open rather
    /// than on the edit after it.
    stamps: HashMap<String, EntryStamps>,
}

/// The size and mtime of everything one pool entry watches: its `.wgsl` and
/// each image it declares, which sit beside it after an eject.
#[derive(Default)]
struct EntryStamps {
    source: Option<(u64, i64)>,
    /// By file name, the way the `@asset` line names it.
    assets: HashMap<String, (u64, i64)>,
}

/// What one sweep of the pool's files turned up.
#[derive(Default)]
struct PoolEdits {
    /// Sources that came back changed, for the caller to approve.
    fresh: Vec<String>,
    /// Whether anything moved at all. An image edit changes no source and
    /// still has to be written back, since writing the pool is what
    /// re-registers every panel wearing the name.
    changed: bool,
}

/// Stat the pool's bookmarked files and pull any edits into their entries.
/// Called from where the surfaces already tick their own watches, so an
/// authoring loop over a pool shader feels the same as one over a panel's
/// own file.
///
/// A source that came back changed is approved, because a reload is a user
/// action: they pointed rox at the file and then edited it. Writing the pool
/// bumps its generation, which is what re-registers every wearer.
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
    // Outside the lock: approving and writing the pool both touch the
    // settings files, and no other panel's paint should wait on that.
    drop(watch);
    if !edits.changed {
        return;
    }
    for source in &edits.fresh {
        approve(source);
    }
    rox_core::settings::set_shader_pool(pool);
}

/// The stat-and-read half of the pool watch, over a pool handed in so a
/// test can point it at files of its own. Writes what moved into `pool` and
/// answers with what needs approving and whether anything moved at all.
///
/// An ejected entry's images sit beside its `.wgsl`, so they're watched the
/// same way, and the list of them comes off the source's own `@asset`
/// lines. That way declaring a new image and dropping the file next to the
/// shader is one save, rather than something the entry has to be taught
/// about first.
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
        // A file that has gone leaves the entry alone, since the source is
        // what runs and the file is only the working copy. The stamp stays
        // put, so the file coming back reads as news.
        if let Some(stamp) = rox_core::settings::file_stamp(&path) {
            if marks.source != Some(stamp) {
                marks.source = Some(stamp);
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if text.trim() != entry.source.trim() {
                        entry.source = text.clone();
                        edits.fresh.push(text);
                        edits.changed = true;
                    }
                }
            }
        }
        let (Some(dir), Ok(spec)) = (path.parent(), parse_chain(&entry.source)) else {
            continue;
        };
        for asset in &spec.assets {
            // The cover binding has no file behind it; the player feeds it.
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
                // The first sweep after an eject stats a file that already
                // says what the entry does, which is nothing to write back.
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
    // Entries that left the pool (a workspace switch, a renamed shader)
    // shouldn't hold a stamp that would make their file's next edit look
    // like old news if the name comes back.
    stamps.retain(|name, _| pool.iter().any(|entry| entry.name == *name));
    edits
}

/// The mtime watch behind hot reload, worn by both shader surfaces: the
/// Shader panel over its own config, and [`PanelSurface`] over a panel's
/// chrome. An external editor plus this is the authoring loop, so it never
/// prompts and never asks for a frame of its own - it rides the paint the
/// shader was already asking for.
#[derive(Default)]
pub struct SourceWatch {
    /// The file's size and mtime when it was last read.
    stamp: Option<(u64, i64)>,
    /// Whether a stamp has been taken for the source in hand. Unseeded, the
    /// first check reads the file whatever the stamp says, so an edit made
    /// while rox was closed lands on open rather than on the edit after it.
    seeded: bool,
    /// The last stat, so the check costs a syscall every
    /// [`RELOAD_EVERY`] rather than one a frame.
    checked: Option<Instant>,
}

impl SourceWatch {
    /// A watch for a source that was just read from `path`, so the next
    /// edit is what wakes it. A source with no file behind it gets an
    /// unseeded watch that never has anything to poll.
    pub fn seeded(path: Option<&Path>) -> SourceWatch {
        SourceWatch {
            stamp: path.and_then(rox_core::settings::file_stamp),
            seeded: path.is_some(),
            checked: Some(Instant::now()),
        }
    }

    /// The file's contents when it has moved since the last look, or None
    /// when it hasn't, when the throttle hasn't elapsed, or when the file
    /// has gone. A file that disappears leaves the running source alone -
    /// that is the whole reason the source is stored inline - and the watch
    /// stays armed for it coming back.
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

/// The target id a route uses to drive slot `n`.
pub fn slot_target(slot: usize) -> String {
    format!("slot{slot}")
}

/// The slot a target id drives, if it names one at all.
pub fn target_slot(id: &str) -> Option<usize> {
    let slot: usize = id.strip_prefix("slot")?.parse().ok()?;
    (slot < SLOTS).then_some(slot)
}

/// Whether a shader says it leaves the surface under it usable, off a bare
/// `// @overlay` line in its source.
///
/// The one question a picker has to answer before somebody hands their whole
/// window to a shader: does the app survive this. Two shapes claim it, and
/// they look nothing alike. Sheen and Lamp are transparent, so the frame
/// blends through them; Dither and Tube are opaque and read `screen`, so they
/// print the frame themselves. What they share is the only thing that
/// matters here, which is that you can still read your library afterwards.
///
/// It can't be derived, which is why it's declared. Binding `screen` proves
/// a shader *can* pass the frame through and nothing more, since a chain is
/// free to sample it and throw it away. Transparency is worse: alpha is a
/// value the fragment stage computes per pixel, so the only way to know is to
/// run it. So the shader's author says, and a shader that says nothing is
/// taken at its most disruptive, which is the safe way to be wrong.
pub fn overlay(source: &str) -> bool {
    source
        .lines()
        .any(|line| chain::directive(line, "@overlay").is_some())
}

/// The slot names a shader declares, read off `// @slot n: name` comments
/// in its source. Anything the source doesn't name comes back None and
/// falls through to [`slot_label`]'s generic wording, so an unannotated
/// shader still binds.
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

/// A slot's display name: what the shader called it, or its number.
pub fn slot_label(labels: &[Option<String>], slot: usize) -> String {
    match labels.get(slot).and_then(|name| name.clone()) {
        Some(name) => name,
        None => format!("slot {slot}"),
    }
}

/// The sixteen slots a route list resolves into, the shader's side of
/// [`RouteTargets`]. Labels only matter to the picker; the paint path
/// builds these bare.
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
    /// Targets that report the shader's own slot names. Only a surface
    /// that lists targets needs these; the panel wrapper resolves routes
    /// without ever asking what a slot is called. The shader panel's
    /// Bindings page is the caller that does.
    pub fn labelled(source: &str) -> Self {
        SlotTargets {
            slots: [0.0; SLOTS],
            labels: slot_labels(source),
        }
    }
}

/// The WGSL accessor a slot arrives on, so a settings page can say where to
/// read it rather than leaving the mapping to be counted out by hand.
pub fn slot_accessor(slot: usize) -> String {
    let lane = ["x", "y", "z", "w"][slot % 4];
    format!("params.signals[{}].{lane}", slot / 4)
}

/// A slot's hand-set value, if one was set. Every surface reads its slots
/// through the resolver, which sees these as seeds, so this is for the
/// tests and for anything asking about one slot alone.
pub fn manual_value(manual: &[(u8, f32)], slot: usize) -> Option<f32> {
    manual
        .iter()
        .find(|(at, _)| *at as usize == slot)
        .map(|(_, value)| *value)
}

/// Set or replace a slot's hand-set value, clamped to the 0..1 every slot
/// carries.
pub fn set_manual_value(manual: &mut Vec<(u8, f32)>, slot: usize, value: f32) {
    let value = value.clamp(0.0, 1.0);
    match manual.iter_mut().find(|(at, _)| *at as usize == slot) {
        Some(entry) => entry.1 = value,
        None => manual.push((slot as u8, value)),
    }
}

/// Lay the hand-set values into the slots before the routes resolve over
/// them: a route wins while it's there, and the hand-set value holds the
/// slot when it isn't. Every shader surface starts a frame this way.
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

/// The workspace state a window's panels feed their shaders from. Panels
/// paint far from any `AppState`, so each window registers its hub and
/// player once and the wrapper looks them up by window, the same
/// window-keyed shape the art tint and the workspace registry use.
#[derive(Default)]
struct ShaderFeeds(HashMap<WindowId, Feed>);

impl Global for ShaderFeeds {}

struct Feed {
    hub: Arc<SignalHub>,
    player: WeakEntity<Player>,
}

/// Register a window's signal hub and player, so any panel painting in it
/// can resolve routes and meta. Called once as the window opens; a second
/// call for the same window replaces the entry. Windows that closed since
/// drop out here, so a stale hub isn't held alive forever.
///
/// Liveness is the window's, not the player's: a popped-out panel shares
/// its parent workspace's player, so closing the popout leaves that player
/// very much alive and its entry would sit here for the rest of the
/// session. The player check stays for the other direction, a window whose
/// workspace went away first.
pub fn note_window(window: &Window, state: &AppState, cx: &mut App) {
    let id = window.window_handle().window_id();
    let live: HashSet<WindowId> = cx.windows().iter().map(|h| h.window_id()).collect();
    let feeds = cx.default_global::<ShaderFeeds>();
    feeds
        .0
        .retain(|window, feed| live.contains(window) && feed.player.upgrade().is_some());
    // The cover feeds ride the same liveness: a closed window's art has
    // nobody left to sample it.
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

/// The playing track's cover art, per window, for the programs binding
/// [`COVER_SOURCE`]. Per window rather than app-global because each
/// workspace window has a player of its own, and two windows sitting on
/// different tracks would otherwise fight over one slot and re-register
/// each other's programs every frame.
struct CoverFeed {
    /// The playing file the image was read for. Two cue tracks of one
    /// image share a path and share their art, so the path is the right
    /// identity here even though it's the wrong one everywhere a track is
    /// named.
    path: Option<PathBuf>,
    /// Bumped when the path turns over, which is what the program keys
    /// hash so a track change re-registers exactly the programs that bind
    /// the art.
    rev: u64,
    image: Option<Arc<AssetImage>>,
}

static COVERS: LazyLock<RwLock<HashMap<u64, CoverFeed>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// The registered cover's cap on its long edge. Registered textures live
/// until the window closes (the patch never evicts), so a session that
/// walks a few hundred albums holds a few hundred of these; at this size
/// that's a megabyte each instead of sixteen, and no shader effect over a
/// panel resolves finer anyway.
const COVER_EDGE: u32 = 512;

/// Point a window's cover feed at the playing file, loading and decoding
/// its art when the path turns over. Answers the feed's revision either
/// way, which is what the caller folds into its program key. Costs a map
/// read and a path compare until the track changes, then one art
/// extraction and decode.
pub fn note_cover(window: u64, path: Option<&Path>) -> u64 {
    {
        let covers = COVERS.read().unwrap();
        if let Some(feed) = covers.get(&window) {
            if feed.path.as_deref() == path {
                return feed.rev;
            }
        } else if path.is_none() {
            // No entry and nothing playing is the fresh-window steady
            // state; writing a rev-0 entry for it would only grow the map.
            return 0;
        }
    }
    // Load outside the lock: another window's paint shouldn't wait on a
    // decode. Paint runs on the main thread, so nobody races the write.
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

/// [`note_cover`] fed from the window's own player: the playing file, or
/// None with the player idle. A window with no feed registered (a child
/// window under the all-windows post shader) leaves its entry alone -
/// [`adopt_cover`] is what fills those.
pub fn poll_cover(window: &Window, cx: &App) -> u64 {
    let id = window.window_handle().window_id().as_u64();
    let Some((_, player)) = window_feed(window, cx) else {
        return COVERS.read().unwrap().get(&id).map_or(0, |feed| feed.rev);
    };
    let path = player
        .read(cx)
        .now_playing()
        .map(|now| now.path().to_path_buf());
    note_cover(id, path.as_deref())
}

/// Copy one window's cover feed onto another, for the child-window sweep:
/// a child wears the primary workspace's program, so it wears its art too.
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

/// What a window's [`COVER_SOURCE`] binding samples, for registration.
pub(crate) fn window_cover(window: u64) -> Option<Arc<AssetImage>> {
    COVERS.read().unwrap().get(&window)?.image.clone()
}

/// A track's front cover as registration-ready pixels, downscaled to
/// [`COVER_EDGE`]. None is a track with no art anywhere, which binds the
/// fallback plate instead.
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

/// The one word gpui answers with on a window whose renderer has no shader
/// pipeline. The patches ride blade's render pipelines, so a DirectX window,
/// or a Mac build without `macos-blade`, turns every registration down with
/// this and nothing about the text is wrong.
const NO_PIPELINE: &str = "unsupported";

/// Whether a registration failure is the backend refusing rather than the
/// shader being broken. The asset step fails first when a program declares
/// an image, so the word arrives prefixed as often as it arrives bare.
pub fn unsupported(error: &str) -> bool {
    error == NO_PIPELINE || error.ends_with(&format!(": {NO_PIPELINE}"))
}

/// What to say instead, since "unsupported" under a "didn't compile"
/// heading sends people hunting through WGSL that's perfectly fine.
pub const NO_PIPELINE_NOTE: &str =
    "This build renders through a backend with no shader pipeline. Shaders ride \
     blade's render pipelines, so every source gets turned down whatever it says.";

/// The headline over that. No full stop: it's a banner headline first, and
/// the panel readout adds one where it reads as a sentence instead.
pub const NO_PIPELINE_TITLE: &str = "Shaders don't run on this build";

/// The last compile message per panel, for its settings window's readout.
/// A panel whose shader compiles clean has no entry.
static ERRORS: LazyLock<RwLock<HashMap<EntityId, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// What a panel's shader last said, or None on a clean compile.
pub fn error(panel: EntityId) -> Option<String> {
    ERRORS.read().unwrap().get(&panel).cloned()
}

/// Store (or clear, with None) a panel's compile message. The paint path
/// writes what registration said; the settings window clears it when the
/// source moves on, and writes its own when a file won't read.
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

/// Sources that failed to compile, keyed by window and source hash. gpui
/// caches successful registrations by content, but a rejection re-runs
/// naga every call, and the wrapper registers from paint - so a broken
/// shader would re-validate on every unrelated repaint without this.
static FAILED: LazyLock<RwLock<HashMap<(u64, u64), String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

fn source_hash(source: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

/// What a failed registration memoizes under: the window, the source, where
/// its images come from, the pool's generation, and the cover feed's.
///
/// The generation rides along because a program can fail over its images
/// rather than its code, and fixing an image changes no source text: the
/// pool watch pulls the new bytes in and bumps the pool, which is what has
/// to clear the memo. Without it a panel would sit on an error nobody could
/// clear short of editing the shader.
///
/// `cover` is the window's cover revision for a source that binds
/// [`COVER_SOURCE`], and zero for the rest, so a track change re-registers
/// exactly the programs wearing the art.
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

/// What a panel's surface is running, per window it draws in. The wrapper
/// paints from an element with nothing but the panel's entity id to hand,
/// so the watch and the last good registration live out here rather than on
/// the panel. Keyed by window as well as panel because a popped-out panel
/// draws in two, and a `UserShaderId` belongs to the window that made it.
struct Live {
    /// The config source this entry was armed for. An edit in the settings
    /// window moves it, which re-arms the watch instead of letting the file
    /// pull the old text back over the edit.
    config: u64,
    watch: SourceWatch,
    /// The file's text, once a reload has moved past the config's copy.
    hot: Option<String>,
    /// The last registration that compiled clean and the program key it
    /// came from. Kept painting while a fresh edit is broken, so an
    /// authoring loop doesn't strobe the panel off and on with every
    /// unfinished save, and skipped past entirely while the key holds
    /// still: a program that hasn't moved has a known id, and splitting it
    /// and decoding its images again every frame would be work with an
    /// answer already in hand.
    good: Option<(u64, UserShaderId)>,
    /// Last time the entry was painted from, so entries for panels that
    /// closed don't hold their sources forever.
    touched: Instant,
}

static LIVE: LazyLock<RwLock<HashMap<(u64, EntityId), Live>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// How long an untouched entry sticks around before the next insert drops
/// it. Long enough that a panel in a background window keeps its state.
const LIVE_TTL: Duration = Duration::from_secs(300);

/// The source a panel's surface is actually running, when a hot reload has
/// moved it past the copy in the config. The settings window folds this back
/// into the config, so a layout saved after an external edit carries the
/// text that was on screen.
pub fn hot_source(panel: EntityId) -> Option<String> {
    LIVE.read()
        .unwrap()
        .iter()
        .find(|((_, id), live)| *id == panel && live.hot.is_some())
        .and_then(|(_, live)| live.hot.clone())
}

/// The render side of a panel's shader, built fresh each render from the
/// chrome and carried by the [`Themed`](super::themed) wrapper.
pub struct PanelSurface {
    source: String,
    /// The pool entry the source came from, so a program declaring images
    /// finds the bytes the look carries for them.
    name: Option<String>,
    /// The file the source was last read from, watched for edits.
    path: Option<PathBuf>,
    routes: Vec<Route>,
    /// The hand-set slot values, seeded under the routes each frame.
    manual: Vec<(u8, f32)>,
    run_when_idle: bool,
    /// The chrome margin, side by side, so the shader covers the panel's
    /// body rect and leaves the gutter the backdrop shows through alone.
    inset: Sides,
}

impl PanelSurface {
    /// The surface a chrome asks for, or None when it carries no runnable
    /// shader - which includes one waiting on approval. An unapproved
    /// source builds no surface at all, so the panel renders exactly as it
    /// would with the shader switched off, and the Shader page is where the
    /// pending source and its Approve button live.
    ///
    /// This is where a pool name resolves. A surface is built once per
    /// render rather than once per frame, and the source it lands on is what
    /// the paint path registers and watches, so resolving here keeps the
    /// lookup off the frame loop and puts the pool's copy through the same
    /// approval gate as an inline one. A name the pool doesn't hold builds
    /// no surface at all.
    pub fn build(chrome: &PanelChrome, margin: Sides) -> Option<PanelSurface> {
        let shader = chrome.shader.as_ref().filter(|s| s.runnable())?;
        let source = resolve_source(shader.name.as_deref(), &shader.source)?;
        if !approved(&source) {
            return None;
        }
        Some(PanelSurface {
            source,
            name: shader.name.clone(),
            // A named surface doesn't watch a file. Its text belongs to the
            // pool, and the panel's own bookmark points at whatever was
            // inlined before the name went on, so reloading it would pull
            // the pool's source back out from under the panel. The pool
            // entry keeps its own bookmark for that.
            path: shader.name.is_none().then(|| shader.path.clone()).flatten(),
            routes: shader.routes.clone(),
            manual: shader.manual.clone(),
            run_when_idle: shader.run_when_idle,
            inset: margin,
        })
    }

    /// Record the shader over the panel's body, after the body itself has
    /// painted. A source that won't compile keeps the last good one on
    /// screen and leaves its message for the panel's settings window;
    /// everything no-ops on a backend without a shader pipeline, which
    /// registration reports the same way.
    pub fn paint(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let panel = window.current_view();
        let window_id = window.window_handle().window_id().as_u64();
        let (source, last_good) = self.current(window_id, panel);
        let ctx = ProgramCtx::of(self.name.as_deref(), self.path.as_deref());
        // A program wearing the track's art follows the track: the poll
        // moves the feed when the playing file turns over, the rev moves
        // the key, and the key re-registers the program with the new art.
        let cover = if uses_cover(&source) {
            poll_cover(window, cx)
        } else {
            0
        };
        let key = program_key(window_id, &source, &ctx, cover);
        let shader = match last_good {
            // Same program as the last frame's, so its id is too. Nothing
            // about it has moved: not the text, not where its images come
            // from, not the pool's generation.
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
        // Nothing has ever compiled here, so there is nothing to keep on
        // screen either. The message is on its way to the settings window.
        let Some(shader) = shader else {
            return;
        };
        let (signals, live) = self.signals(window, cx);
        let meta = meta_slots(window, cx);
        let bounds = body_rect(bounds, self.inset);
        // Caps decide the path: a program that reads the screen under it,
        // its own last frame, an image, or runs more than one pass needs the
        // region pass, and one that draws from nothing but its uniforms is a
        // plain in-scene quad. Getting this backwards paints nothing at all,
        // since each call skips what it can't run.
        let screen = window
            .user_shader_caps(shader)
            .is_some_and(|caps| caps.screen_pass_only());
        if screen {
            window.paint_screen_shader(bounds, shader, panel.as_u64(), signals, meta);
        } else {
            window.paint_user_shader(bounds, shader, signals, meta);
        }
        // Docked panels render cached: a clean frame replays the recorded
        // primitive with the values it was recorded with, so an animating
        // shader needs its panel dirtied every frame. `request_animation_frame`
        // notifies exactly this view, which is the cheap wake - a window
        // `refresh` would rebuild every view in the window uncached and
        // stall the whole frame loop.
        if live || self.run_when_idle {
            window.request_animation_frame();
        }
    }

    /// The source to run this frame and the last one that compiled, taking
    /// the hot reload with it: the watch stats the config's file every
    /// [`RELOAD_EVERY`], and a file that has moved becomes what runs until
    /// the settings window folds it back into the config.
    ///
    /// The reload only happens for a surface that is already painting, which
    /// means already approved. A pending source never gets here, so a bundle
    /// can't have rox read a path of its choosing and trust what comes back.
    fn current(&self, window: u64, panel: EntityId) -> (String, Option<(u64, UserShaderId)>) {
        // The pool's watch rides along here: it's throttled and app-wide, so
        // whichever surface paints first in a frame pays for it and the rest
        // cost an elapsed check. A named surface doesn't watch a file of its
        // own, and this is what gives it hot reload anyway.
        poll_pool();
        let config = source_hash(&self.source);
        let mut fresh = None;
        let (source, good) = {
            let mut live = LIVE.write().unwrap();
            let entry = match live.entry((window, panel)) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    // A panel restored from a layout has a source snapshot
                    // and maybe a path, with no telling whether they still
                    // agree, so its watch starts unseeded and reads once.
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
                // The settings window wrote a new source; it wins over
                // whatever the file said, and the watch re-arms from it.
                entry.config = config;
                entry.hot = None;
                entry.watch = SourceWatch::seeded(self.path.as_deref());
            }
            entry.touched = Instant::now();
            if let Some(path) = &self.path {
                if let Some(text) = entry.watch.poll(path) {
                    let running = entry.hot.as_deref().unwrap_or(&self.source);
                    if text.trim() != running.trim() {
                        // The user pointed rox at this file, so what comes
                        // out of it is theirs; approving here is what keeps
                        // the edit from tripping the gate on restart.
                        fresh = Some(text.clone());
                        entry.hot = Some(text);
                    }
                }
            }
            (
                entry.hot.clone().unwrap_or_else(|| self.source.clone()),
                entry.good,
            )
        };
        // Outside the lock: approving writes the settings file, and no other
        // panel's paint should wait on that.
        if let Some(text) = fresh {
            approve(&text);
        }
        (source, good)
    }

    /// Remember a clean registration as this surface's fallback, and drop
    /// the entries of panels that stopped drawing a while back.
    fn note_good(&self, window: u64, panel: EntityId, key: u64, shader: UserShaderId) {
        let mut live = LIVE.write().unwrap();
        if let Some(entry) = live.get_mut(&(window, panel)) {
            entry.good = Some((key, shader));
        }
        if live.len() > 32 {
            let now = Instant::now();
            live.retain(|_, entry| now.duration_since(entry.touched) < LIVE_TTL);
        }
    }

    /// This frame's slot values, and whether the hub is moving. Moving
    /// covers the release too: the audio stopping is where a smoothed
    /// signal starts falling, so a surface that parked on the last live
    /// frame would hold the fade halfway down. The tick happens here
    /// because a panel shader can be the only thing in the window watching
    /// the audio; it's deduped inside the hub, so several shaded panels
    /// cost one.
    fn signals(&self, window: &Window, cx: &App) -> ([f32; SLOTS], bool) {
        // The hand-set values first, routes written over them: a slot
        // nothing routes holds the knob the Shader page set, and a routed
        // one is the route's outright.
        let mut targets = SlotTargets::default();
        seed_manual(&mut targets, &self.manual);
        let Some((hub, player)) = window_feed(window, cx) else {
            return (targets.slots, false);
        };
        {
            let player = player.read(cx);
            hub.tick(&player.feed(), player.playing_entry());
        }
        signal_ui::apply_routes(&self.routes, &hub, &mut targets);
        (targets.slots, hub.live() || hub.settling())
    }
}

/// The panel's body rect: the wrapper's bounds pulled in by the chrome
/// margin, side by side, so the shader covers what the panel draws and
/// not the gap around it. Each inset stops at half its axis, so a margin
/// wider than the panel closes the rect instead of turning it inside out.
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

fn window_feed(window: &Window, cx: &App) -> Option<(Arc<SignalHub>, gpui::Entity<Player>)> {
    let feeds = cx.try_global::<ShaderFeeds>()?;
    let feed = feeds.0.get(&window.window_handle().window_id())?;
    Some((feed.hub.clone(), feed.player.upgrade()?))
}

/// The eight `meta` floats every rox shader can count on, the convention
/// the Shader panel shares: volume, where the track sits, whether audio is
/// moving, how long the track runs, how dark the theme renders, and which
/// theme the user actually picked. The last two are reserved and read
/// zero, so a shader written against them today keeps working when they
/// fill in.
pub fn meta_slots(window: &Window, cx: &App) -> [f32; 8] {
    let mut meta = [0.0f32; 8];
    // The active palette's root background as luma, 0 pitch black to 1
    // paper white, so one shader can tune itself to both themes instead of
    // shipping for the one it was written against. Set before the feed
    // check: the theme is known even in a window no player registered.
    let bg = rox_design::palette::bg_root_opaque();
    meta[4] = (0.2126 * bg.r + 0.7152 * bg.g + 0.0722 * bg.b).clamp(0.0, 1.0);
    // The theme pick itself, same polarity as the luma beside it: 1 light,
    // 0 dark. Not the same fact as slot 4 and not derivable from it - song
    // theming can swap the rendered side out from under a cover, and it
    // moves the luma around within a side too, so a shader that wants a
    // clean "which theme am I in" reads this and a shader that wants "how
    // bright is the page right now" reads slot 4.
    meta[5] = match rox_design::palette::mode() {
        rox_design::palette::Mode::Light => 1.0,
        rox_design::palette::Mode::Dark => 0.0,
    };
    let Some((_, player)) = window_feed(window, cx) else {
        return meta;
    };
    let player = player.read(cx);
    // The persisted volume runs to 200%; the slot is documented 0..1, so a
    // boosted level reads as full rather than pushing the slot past it.
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

/// The shader pool is app-global, and the tests in this file and in
/// [`chain`] both swap it out from under themselves. Anything that touches
/// it takes this first, so a parallel run doesn't have one test asserting
/// against another's pool.
#[cfg(test)]
static POOL_GUARD: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use rox_viz::signal::Source;
    use rox_viz::AudioFeed;

    #[test]
    fn slot_targets_round_trip() {
        for slot in 0..SLOTS {
            assert_eq!(target_slot(&slot_target(slot)), Some(slot));
        }
        assert_eq!(target_slot("slot16"), None);
        assert_eq!(target_slot("bass"), None);
        assert_eq!(target_slot(""), None);
    }

    /// Every way a config can arrive at a shader, and what the picker's
    /// closed state calls it. This is the read both settings pages hang
    /// their whole layout off, so it gets checked here rather than left to
    /// whichever page someone happens to open.
    #[test]
    fn a_config_lands_on_the_picker_entry_it_came_from() {
        // Nothing at all, and a source that's only whitespace, which is
        // what a cleared panel leaves behind.
        assert_eq!(pick(None, None, None), Pick::Empty);
        assert_eq!(pick(None, None, Some("  \n ")), Pick::Empty);
        assert_eq!(pick_label(&Pick::Empty), "None");

        // A shipped example, matched on the text the way the old chips did.
        assert_eq!(pick(None, None, Some(PLASMA)), Pick::Example(0));
        assert_eq!(pick(None, None, Some(TRAILS)), Pick::Example(1));
        // Trailing whitespace is an editor's doing, not a different shader.
        assert_eq!(
            pick(None, None, Some(&format!("{PLASMA}\n\n"))),
            Pick::Example(0)
        );
        assert_eq!(pick_label(&Pick::Example(0)), "Plasma");
        assert_eq!(pick_label(&Pick::Example(1)), "Trails");

        // A file, which beats the example match: ejecting Plasma to a file
        // leaves the text identical, and from there the file is what's
        // being edited.
        let path = PathBuf::from("/home/someone/shaders/grain.wgsl");
        assert_eq!(pick(None, Some(&path), Some(PLASMA)), Pick::File(path));
        assert_eq!(
            pick_label(&Pick::File("/home/someone/shaders/grain.wgsl".into())),
            "grain"
        );
        // No stem to show falls back to the whole path rather than to
        // nothing at all.
        assert_eq!(pick_label(&Pick::File("/".into())), "/");

        // Source of its own that matches nothing, which is what arrives
        // inside a layout or a bundle.
        assert_eq!(pick(None, None, Some("// mine")), Pick::Custom);
        assert_eq!(pick_label(&Pick::Custom), "Custom");

        // A workspace shader. The name wins over everything the config
        // still carries inline, and a name that resolves to nothing reads
        // as missing rather than as empty.
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

    /// The cover feed turns over with the playing file, not with the
    /// frames asking about it, and the child sweep's adoption follows the
    /// same rule.
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
        // Tracks nothing holds art for read as no image, which is what
        // binds the fallback plate at registration.
        assert!(window_cover(window).is_none());

        note_cover(window, Some(&a));
        adopt_cover(window, child);
        let rev = |id: u64| COVERS.read().unwrap().get(&id).map(|feed| feed.rev);
        assert_eq!(rev(child), Some(1));
        adopt_cover(window, child);
        assert_eq!(rev(child), Some(1), "adopting the same art moves nothing");
    }

    /// Every example carries its own line for the page to print, so adding
    /// one stays a file plus a row in the table.
    #[test]
    fn every_example_brings_its_own_blurb() {
        for (index, preset) in PRESETS.iter().enumerate() {
            assert!(
                !preset.blurb.trim().is_empty(),
                "{} ships without a line to print under it",
                preset.label
            );
            assert_eq!(pick_blurb(index), preset.blurb);
        }
        assert_eq!(pick_blurb(PRESETS.len()), "");
    }

    /// The name wins where it resolves, and where it doesn't the surface
    /// gets nothing rather than the stale inline copy: a name says "whatever
    /// the workspace calls this", and running something else under it would
    /// be worse than a blank panel.
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

        // Leave the pool as it was found, since it's app-global and the
        // other tests in this binary share it.
        rox_core::settings::note_shader_pool(Vec::new());
        assert_eq!(resolve_source(Some("Grain"), "// the panel's own"), None);
    }

    /// A config with a name has something to run even with no source of its
    /// own, which is the whole point of pointing at the pool. Whether the
    /// pool actually holds it is registration's question, not the config's.
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

    /// A shader that names the pool writes the name, and one that doesn't
    /// writes no key, so no layout dump written before the pool existed
    /// grows a line.
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

        // A dump from before names existed reads as an inline shader.
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

    /// A hub carrying one band signal, run up to full off a tone in that
    /// band. The engine's attack takes a stretch of wall clock (the tick
    /// throttles), so this walks it there rather than faking a value.
    fn loud_hub() -> (SignalHub, u64) {
        let hub = SignalHub::new(Vec::new());
        let (id, _) = hub.add(
            Source::Band {
                lo: 800.0,
                hi: 2000.0,
            },
            0.0,
        );
        let feed = AudioFeed::new();
        // 1.17 kHz at 48 kHz, the midrange tone the engine's own tests use.
        let mut phase = 0.0f32;
        for _ in 0..60 {
            let mut samples = vec![0.0f32; 4096];
            for frame in samples.chunks_mut(2) {
                phase += std::f32::consts::TAU * 1170.0 / 48_000.0;
                frame[0] = phase.sin();
                frame[1] = frame[0];
            }
            feed.push(&samples);
            hub.tick(&feed, None);
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
            // Half the span, so the same signal lands at half strength.
            route(loud, slot_target(5), 0.0, 0.5, true),
            // Off, so slot 7 stays at rest.
            route(loud, slot_target(7), 0.0, 1.0, false),
            // A signal the pool never carried contributes nothing.
            route(999, slot_target(9), 0.0, 1.0, true),
            // A target nothing answers to is skipped, not a panic.
            route(loud, "nowhere".to_string(), 0.0, 1.0, true),
            // Out of range reads as no slot at all.
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

    /// A source no list will ever carry, unique per call so two tests
    /// approving at once can't see each other's.
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
        // What the Approve button does, minus the settings write (which
        // would land in the machine's real session file).
        let print = fingerprint(&source);
        assert!(rox_core::settings::note_approved(&print));
        assert!(approved(&source), "an approved hash runs");
        // The same program with a different name in it is a different
        // program, and doesn't ride the first one's approval.
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
        // Approving a builtin is a no-op rather than a list entry, so the
        // file doesn't fill up with hashes of what shipped.
        approve(PLASMA);
        assert!(!rox_core::settings::shader_approved(&fingerprint(PLASMA)));
        // Nothing to run is nothing to gate.
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
        // Hex of a SHA-256, so the list stays readable and fixed width.
        let print = fingerprint(source);
        assert_eq!(print.len(), 64);
        assert!(print.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn a_builtin_survives_a_round_trip_through_a_layout() {
        // Presets ride a dump as inline source like anything else, and
        // serde's string round trip is where a trailing newline would go
        // missing. The gate has to still know it as ours on the way back.
        let dumped = serde_json::to_string(&PLASMA.to_string()).expect("dump");
        let read: String = serde_json::from_str(&dumped).expect("read");
        assert!(approved(&read));
    }

    /// A file to watch, in a directory of this test's own so a parallel
    /// test run can't stat somebody else's writes.
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
        // Unseeded, so an edit made while rox was closed lands on open
        // rather than on the edit after it.
        assert_eq!(watch.poll(&path).as_deref(), Some("one"));
        // Throttled: the next look inside the window costs no syscall and
        // reports nothing.
        assert_eq!(watch.poll(&path), None);
        watch.checked = None;
        assert_eq!(watch.poll(&path), None, "nothing moved");
        // The stamp is size and mtime, and mtime only resolves to the
        // second, so the change here is a length.
        watch.checked = None;
        std::fs::write(&path, "one two three").expect("rewrite");
        assert_eq!(watch.poll(&path).as_deref(), Some("one two three"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_seeded_watch_waits_for_the_next_edit() {
        let path = scratch("seeded");
        std::fs::write(&path, "one").expect("write");
        // What a file pick leaves behind: the source was just read from
        // here, so the file as it stands is not news.
        let mut watch = SourceWatch::seeded(Some(path.as_path()));
        watch.checked = None;
        assert_eq!(watch.poll(&path), None);
        watch.checked = None;
        std::fs::write(&path, "one two three").expect("rewrite");
        assert_eq!(watch.poll(&path).as_deref(), Some("one two three"));
        // A file that goes missing leaves the running source alone and the
        // watch armed for it coming back.
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

    /// The eject folder this test writes into, emptied first so a rerun
    /// starts from nothing.
    fn eject_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("rox-shader-eject-{name}"));
        std::fs::remove_dir_all(&root).ok();
        root
    }

    /// The collision rule: the same shader keeps its file, a different one
    /// under a taken name slides down to a numbered variant rather than
    /// writing over somebody's edits.
    #[test]
    fn ejecting_over_a_diverged_file_takes_the_next_name() {
        let root = eject_root("collision");
        let named = |stem: &str| rox_core::settings::shader_eject_path_in(&root, "Nightfall", stem);

        let first = eject_in(&root, "Nightfall", "Grain", "// one", &[]).expect("eject");
        assert_eq!(first, named("Grain"));
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "// one");

        // The same shader, give or take the newline an editor leaves: the
        // file already says it, so it keeps its name.
        let again = eject_in(&root, "Nightfall", "Grain", "\n// one\n", &[]).expect("eject");
        assert_eq!(again, first);

        // A different shader under a taken name gets its own file, and the
        // one that was there is left exactly as it was.
        let second = eject_in(&root, "Nightfall", "Grain", "// two", &[]).expect("eject");
        assert_eq!(second, named("Grain-2"));
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "\n// one\n");

        let third = eject_in(&root, "Nightfall", "Grain", "// three", &[]).expect("eject");
        assert_eq!(third, named("Grain-3"));
        // And the one that already has a numbered file lands back on it
        // rather than walking further down every time.
        assert_eq!(
            eject_in(&root, "Nightfall", "Grain", "// two", &[]).expect("eject"),
            named("Grain-2")
        );

        // Names double as path components, so a name that can't be one
        // folds through the shared sanitizer.
        let awkward = eject_in(&root, "Nightfall", "a/b", "// slashed", &[]).expect("eject");
        assert_eq!(awkward, named("a b"));
        std::fs::remove_dir_all(&root).ok();
    }

    /// A bookmarked pool entry follows its file, and one without a bookmark
    /// is never read for.
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

        // The first sweep reads the file whatever the stamp says, and it
        // holds what the entry does, so nothing moves.
        assert!(!pool_reload(&mut stamps, &mut pool).changed);

        std::fs::write(&path, "// one, and then some more").expect("rewrite");
        assert_eq!(
            pool_reload(&mut stamps, &mut pool).fresh,
            vec!["// one, and then some more".to_string()]
        );
        assert_eq!(pool[0].source, "// one, and then some more");
        // Quiet again once the stamp has caught up, and the entry with no
        // file behind it was never in play.
        assert!(!pool_reload(&mut stamps, &mut pool).changed);
        assert_eq!(pool[1].source, "// bloom");

        // An entry that leaves the pool drops its stamp, so the same file
        // coming back under that name reads as news rather than old.
        assert!(!pool_reload(&mut stamps, &mut Vec::new()).changed);
        assert!(stamps.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A 1x1 PNG, and a second one that differs by a byte, so an edit is
    /// something the stamp can see.
    fn plate(red: u8) -> Vec<u8> {
        let mut image = image::RgbaImage::new(1, 1);
        image.put_pixel(0, 0, image::Rgba([red, 0, 0, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("encode");
        bytes.into_inner()
    }

    /// Eject writes a shader's images beside its `.wgsl`, so the authoring
    /// loop is an editor for the code and an image editor for the plates.
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

        // Images overwrite rather than sliding down to a variant: the file
        // name is what the shader binds by, so a numbered copy would only
        // leave the shader sampling the old plate.
        std::fs::write(&beside, b"scribbled on").expect("write");
        let again = eject_in(&root, "Nightfall", "Stamp", source, &assets).expect("eject");
        assert_eq!(again, path);
        assert_eq!(std::fs::read(&beside).unwrap(), plate(200));

        // An entry hand-edited into something that isn't base64 reads out
        // here rather than at the next registration.
        let broken = vec![rox_core::settings::ShaderAsset {
            file: "plate.png".to_string(),
            data: "not base64 at all!!".to_string(),
        }];
        assert!(eject_in(&root, "Nightfall", "Stamp", source, &broken).is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    /// The watch stats a bookmarked entry's images too, and the list of them
    /// comes off the source's own `@asset` lines: declaring a new image and
    /// dropping the file next to the shader is one save.
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

        // The first sweep takes the image the entry never carried, which is
        // an ejected shader growing a plate.
        let edits = pool_reload(&mut stamps, &mut pool);
        assert!(edits.changed, "a new image is news");
        assert!(edits.fresh.is_empty(), "and it approves nothing: it's data");
        assert_eq!(pool[0].assets.len(), 1);
        assert_eq!(pool[0].assets[0].file, "plate.png");
        assert_eq!(pool[0].assets[0].decode().unwrap(), plate(10));

        // Quiet once the stamp has caught up.
        assert!(!pool_reload(&mut stamps, &mut pool).changed);

        // An edit in an image editor lands the same way a shader edit does.
        // The stamp is size and mtime and mtime only resolves to the second,
        // so the change here is a length.
        std::fs::write(dir.join("plate.png"), [plate(10), plate(240)].concat()).expect("rewrite");
        assert!(pool_reload(&mut stamps, &mut pool).changed);
        assert_eq!(
            pool[0].assets[0].decode().unwrap(),
            [plate(10), plate(240)].concat()
        );

        // A file the source stopped declaring is left where it is, and an
        // entry with no bookmark is never stat'd for one.
        std::fs::write(&path, "fn fs_user() {}").expect("rewrite");
        let edits = pool_reload(&mut stamps, &mut pool);
        assert_eq!(edits.fresh, vec!["fn fs_user() {}".to_string()]);
        assert_eq!(pool[0].assets.len(), 1, "the bytes stay with the entry");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// An inline shader ejects under its panel's name, and a panel with
    /// nothing to call it by falls back to the source's own hash.
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
        // Each side pulls in on its own, and only its own edge moves.
        let lopsided = body_rect(
            bounds,
            Sides::ZERO.with(rox_design::palette::Side::Left, 8.0),
        );
        assert_eq!(lopsided.origin.x, gpui::px(18.));
        assert_eq!(lopsided.origin.y, gpui::px(20.));
        assert_eq!(lopsided.size.width, gpui::px(92.));
        assert_eq!(lopsided.size.height, gpui::px(50.));
        // A margin wider than the panel can't invert the rect.
        let squeezed = body_rect(bounds, Sides::all(400.0));
        assert!(squeezed.size.width >= gpui::px(0.));
        assert!(squeezed.size.height >= gpui::px(0.));
    }
}
