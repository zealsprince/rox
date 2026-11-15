//! Named workspace bundles: a whole shareable look (layout presets, palette,
//! appearance) under a name. Two sources: the user's files under
//! [`settings::workspaces_dir`], and the bundles shipped in the app's assets.
//!
//! A saved workspace is one `.roxw` archive, so dropping a shared file in the
//! folder adds it. The list reads names off the filenames and only reads a
//! bundle when its contents are needed, so a menu flyout never opens every
//! archive per frame. Applying one swaps its files into the live asset
//! folder before the look itself changes.
//!
//! To ship one: export it from the settings Workspace page, put its
//! `workspace.json` in `assets/workspaces/` under the workspace's name, and
//! its `assets/` folder beside it as `<Name>/assets/`.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use gpui::{App, SharedString};

use rox_core::settings::assets as asset_store;
use rox_core::settings::panel_presets::{self, PresetEntry, PresetScope};
use rox_core::settings::roxw::{self, Drained};
use rox_core::settings::{
    self, Asset, LookState, NamedShader, Settings, WORKSPACE_VERSION, WorkspaceBundle,
};
use rox_design::assets;
use rox_design::palette::{self, Palette};
use rox_design::tokens;
use rox_panel_api::panel::shader;

/// Building the list costs a directory read and nothing more.
pub struct Entry {
    /// The lookup key, never translated, so a settings file resolves in any
    /// language.
    pub name: String,
    /// Translated only for shipped bundles named with a word rather than a
    /// proper name.
    pub title: SharedString,
    pub path: Option<PathBuf>,
    pub builtin: bool,
    /// Only shipped entries carry it, since their bundles are parsed anyway;
    /// the settings page fills the saved side from [`saved_authors`].
    pub author: Option<String>,
    /// One per theme side (see [`assets::workspace_preview`]).
    pub preview_dark: Option<SharedString>,
    pub preview_light: Option<SharedString>,
}

/// Where a resolved workspace's files are read from.
#[derive(Clone, Debug, PartialEq)]
pub enum Origin {
    /// A `.roxw` in the workspaces folder.
    Saved(PathBuf),
    /// Shipped in the app, by its file stem.
    Shipped(String),
}

/// Refuses a newer format; names a nameless bundle after its file.
fn read_file(path: &Path) -> Option<WorkspaceBundle> {
    let mut bundle = match roxw::read_bundle(path) {
        Ok(bundle) => bundle,
        Err(e) => {
            log::debug!("workspace: skipping {}: {e}", path.display());
            return None;
        }
    };
    if bundle.name.trim().is_empty() {
        bundle.name = stem_of(path)?;
    }
    Some(bundle)
}

fn stem_of(path: &Path) -> Option<String> {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.trim().is_empty())
}

/// The slug keying a shipped bundle's name and blurb in the locale files.
/// Kept there rather than as a schema field, since third parties author
/// bundles; a bundle rox doesn't ship keeps its author's own text.
fn shipped_slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.ends_with('-') {
            // One separator per run, so "Llama (WinAmp)" doesn't get a double hyphen.
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

pub fn display_title(name: &str) -> String {
    rox_i18n::try_translate(&format!("workspace-shipped-{}", shipped_slug(name)))
        .map(|t| t.to_string())
        .unwrap_or_else(|| name.to_string())
}

pub fn display_blurb(name: &str, own: &str) -> Option<SharedString> {
    if let Some(text) =
        rox_i18n::try_translate(&format!("workspace-shipped-{}-blurb", shipped_slug(name)))
    {
        return Some(text);
    }
    Some(own.trim())
        .filter(|d| !d.is_empty())
        .map(|d| SharedString::from(d.to_string()))
}

/// With its file stem, which keys its pictures and its asset folder.
fn shipped_bundle(name: &str) -> Option<(WorkspaceBundle, String)> {
    assets::shipped_workspaces()
        .into_iter()
        .find_map(|(stem, bytes)| {
            let mut bundle = serde_json::from_slice::<WorkspaceBundle>(&bytes).ok()?;
            if bundle.version > WORKSPACE_VERSION {
                return None;
            }
            if bundle.name.trim().is_empty() {
                bundle.name = stem.clone();
            }
            (bundle.name == name).then_some((bundle, stem))
        })
}

/// Skips any file that's from a newer format, doesn't parse, or has no
/// usable name. Sorted by name.
pub fn shipped() -> Vec<Entry> {
    let mut out: Vec<Entry> = assets::shipped_workspaces()
        .into_iter()
        .filter_map(|(stem, bytes)| {
            let bundle = serde_json::from_slice::<WorkspaceBundle>(&bytes).ok()?;
            if bundle.version > WORKSPACE_VERSION {
                return None;
            }
            // The pictures are keyed by file stem, not the bundle's name.
            let preview_dark = assets::workspace_preview(&stem, palette::Mode::Dark);
            let preview_light = assets::workspace_preview(&stem, palette::Mode::Light);
            let author = Some(bundle.meta.author.clone()).filter(|a| !a.trim().is_empty());
            let name = match bundle.name.trim() {
                "" => stem,
                named => named.to_string(),
            };
            (!name.trim().is_empty()).then_some(Entry {
                title: display_title(&name).into(),
                name,
                path: None,
                builtin: true,
                preview_dark,
                preview_light,
                author,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Named after the file rather than the bundle inside, so the list is one
/// directory read and a hand-edited name doesn't disagree with the disk.
pub fn saved() -> Vec<Entry> {
    saved_in(&settings::workspaces_dir())
}

fn saved_in(dir: &Path) -> Vec<Entry> {
    let Ok(dir) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Entry> = dir
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| is_archive(path))
        .filter_map(|path| {
            let name = stem_of(&path)?;
            Some(Entry {
                title: name.clone().into(),
                name,
                path: Some(path),
                builtin: false,
                preview_dark: None,
                preview_light: None,
                author: None,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The one place the saved list parses its bundles; callers read it once
/// and keep the answer.
pub fn saved_authors() -> BTreeMap<String, String> {
    saved_authors_in(&settings::workspaces_dir())
}

fn saved_authors_in(dir: &Path) -> BTreeMap<String, String> {
    saved_in(dir)
        .into_iter()
        .filter_map(|entry| {
            let bundle = read_file(entry.path.as_ref()?)?;
            let author = bundle.meta.author.trim();
            (!author.is_empty()).then(|| (entry.name, author.to_string()))
        })
        .collect()
}

/// The `kind` presets saved in every other workspace, read-only and detached
/// from their own bundle's pool so they run in this one. One manifest read
/// per saved archive, at menu open, and nothing extracted.
pub fn presets_elsewhere(kind: &str) -> Vec<PresetEntry> {
    let live = Settings::load().look.bundle.name;
    presets_elsewhere_in(&settings::workspaces_dir(), &live, kind)
}

fn presets_elsewhere_in(dir: &Path, live: &str, kind: &str) -> Vec<PresetEntry> {
    saved_in(dir)
        .into_iter()
        .filter(|entry| entry.name != live)
        .filter_map(|entry| Some((read_file(entry.path.as_ref()?)?, entry.name)))
        .flat_map(|(bundle, name)| {
            bundle
                .panel_presets
                .iter()
                .filter(|preset| preset.panel_name() == Some(kind))
                .map(|preset| PresetEntry {
                    scope: PresetScope::Other(name.clone()),
                    preset: panel_presets::detach(preset, &bundle.shaders),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Keeps the card an existing save under that name already has (see
/// [`WorkspaceMeta::carry_forward`](rox_core::settings::WorkspaceMeta::carry_forward)).
/// Only saved bundles are looked up: saving under a shipped name is a fork,
/// and a fork shouldn't arrive signed by the original's author.
pub fn snapshot(name: &str, s: &Settings) -> WorkspaceBundle {
    snapshot_in(&settings::workspaces_dir(), name, s)
}

fn snapshot_in(dir: &Path, name: &str, s: &Settings) -> WorkspaceBundle {
    let mut bundle = WorkspaceBundle::from_settings(name.to_string(), s);
    if let Some(prior) = read_file(&file_of_in(dir, name)) {
        bundle.meta.carry_forward(&prior.meta);
    }
    bundle
}

/// Trust every shader in the shipped workspaces at startup, before a window
/// can paint one. Installing rox is the agreement; without this a shipped
/// look's panels would come up blank asking for approval.
pub fn trust_shipped_shaders() {
    let prints = assets::shipped_workspaces()
        .into_iter()
        .filter_map(|(_, bytes)| serde_json::from_slice::<WorkspaceBundle>(&bytes).ok())
        .filter(|bundle| bundle.version <= WORKSPACE_VERSION)
        .flat_map(|bundle| bundle_fingerprints(&bundle));
    settings::trust_shipped(prints);
}

fn bundle_fingerprints(bundle: &WorkspaceBundle) -> Vec<String> {
    bundle
        .shaders
        .iter()
        .map(|shader| shader.source.clone())
        .chain(bundle.post_shader.iter().map(|post| post.source.clone()))
        .chain(
            bundle
                .backdrop_shader
                .iter()
                .map(|post| post.source.clone()),
        )
        .chain(
            bundle
                .layouts
                .iter()
                .flat_map(|layout| settings::dump_shader_sources(&layout.dump)),
        )
        .filter(|source| !source.trim().is_empty())
        .map(|source| shader::fingerprint(&source))
        .collect()
}

pub struct PendingShader {
    /// The pool entry's name, or the head of the hash for inline-only code.
    pub label: String,
    pub source: String,
}

/// Every distinct unapproved shader in a bundle, pool first so a shared
/// source takes its pool name.
///
/// A panel that names a pool entry still holds its old inline source, which
/// the dump walk can't tell apart, so a stale copy gets listed. That's the
/// safe way round: it's still code inside the bundle.
pub fn unapproved_shaders(bundle: &WorkspaceBundle) -> Vec<PendingShader> {
    let named = bundle
        .shaders
        .iter()
        .map(|shader| (Some(shader.name.clone()), shader.source.clone()));
    let screen = bundle
        .post_shader
        .iter()
        // A pool name runs the pool's source, already listed above.
        .filter(|post| {
            post.name
                .as_deref()
                .is_none_or(|name| !bundle.shaders.iter().any(|shader| shader.name == name))
        })
        .map(|post| (None, post.source.clone()));
    let backdrop = bundle
        .backdrop_shader
        .iter()
        .filter(|post| {
            post.name
                .as_deref()
                .is_none_or(|name| !bundle.shaders.iter().any(|shader| shader.name == name))
        })
        .map(|post| (None, post.source.clone()));
    let dumps = bundle_dumps(bundle)
        .flat_map(settings::dump_shader_sources)
        .map(|source| (None, source));

    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (name, source) in named.chain(screen).chain(backdrop).chain(dumps) {
        if shader::approved(&source) {
            continue;
        }
        let print = shader::fingerprint(&source);
        if !seen.insert(print.clone()) {
            continue;
        }
        let label = name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| print[..8].to_string());
        out.push(PendingShader { label, source });
    }
    out
}

/// None when nothing would actually run. Separate from
/// [`unapproved_shaders`], which goes quiet once code is approved; this is
/// about the look, and a screen shader covers the whole window regardless.
fn screen_shader_line(bundle: &WorkspaceBundle) -> Option<SharedString> {
    let post = bundle.post_shader.as_ref().filter(|post| post.enabled)?;
    // The runtime's resolution: a pool name wins, exactly or not at all.
    if let Some(name) = post.name.as_deref() {
        return bundle
            .shaders
            .iter()
            .any(|entry| entry.name == name)
            .then(|| rox_i18n::t!("workspace-apply-screen-shader-named", name = name));
    }
    (!post.source.trim().is_empty()).then(|| rox_i18n::t!("workspace-apply-screen-shader-plain"))
}

/// What the apply confirm's two yeses hang off. Deliberately not keyed on
/// trust: [`unapproved_shaders`] goes quiet once code is approved, and a
/// look would then bring its shaders along without asking. The pool alone
/// doesn't count; nothing in it paints until a surface points at it.
pub fn wears_shaders(bundle: &WorkspaceBundle) -> bool {
    screen_shader_line(bundle).is_some()
        || backdrop_shader_runs(bundle)
        || bundle_dumps(bundle).any(settings::dump_wears_shader)
}

fn backdrop_shader_runs(bundle: &WorkspaceBundle) -> bool {
    let Some(config) = bundle.backdrop_shader.as_ref().filter(|c| c.enabled) else {
        return false;
    };
    match config.name.as_deref() {
        Some(name) => bundle.shaders.iter().any(|entry| entry.name == name),
        None => !config.source.trim().is_empty(),
    }
}

/// Layouts and panel presets both, since a preset's panel takes a shader
/// the same way.
fn bundle_dumps(bundle: &WorkspaceBundle) -> impl Iterator<Item = &serde_json::Value> {
    bundle
        .layouts
        .iter()
        .map(|layout| &layout.dump)
        .chain(bundle.panel_presets.iter().map(|preset| &preset.panel))
}

/// The look with every shader switched off rather than removed, for the
/// confirm's Without Shaders. Each piece stays one toggle away.
pub fn without_shaders(bundle: &WorkspaceBundle) -> WorkspaceBundle {
    let mut bare = bundle.clone();
    if let Some(post) = bare.post_shader.as_mut() {
        post.enabled = false;
    }
    if let Some(backdrop) = bare.backdrop_shader.as_mut() {
        backdrop.enabled = false;
    }
    for layout in &mut bare.layouts {
        settings::strip_dump_shaders(&mut layout.dump);
    }
    for preset in &mut bare.panel_presets {
        settings::strip_dump_shaders(&mut preset.panel);
    }
    bare
}

/// Built once when the dialog opens: a confirm paints every frame, and what's
/// behind it is a file read and a page of JSON.
pub struct ApplyCard {
    pub name: String,
    pub byline: Option<SharedString>,
    pub description: Option<SharedString>,
    pub shaders: Vec<PendingShader>,
    pub screen_shader: Option<SharedString>,
    /// Splits the dialog's yes in two, every time. See [`wears_shaders`].
    pub wears_shaders: bool,
    /// The plugins the look's panels came from that aren't running here.
    pub requires: Option<SharedString>,
    /// The unpacked size, said only past [`roxw::WARN_BYTES`]. A heavy look
    /// still applies.
    pub heavy: Option<SharedString>,
}

fn heavy_line(bundle: &WorkspaceBundle) -> Option<SharedString> {
    let unpacked: u64 = bundle.assets.iter().map(|asset| asset.bytes).sum();
    if unpacked <= roxw::WARN_BYTES {
        return None;
    }

    // Decimal, since the string says MB.
    let mb = unpacked as f64 / 1_000_000.0;
    Some(rox_i18n::t!(
        "settings-workspace-heavy",
        size = format!("{mb:.0}")
    ))
}

/// Informational: the look applies either way, and its plugin panels restore
/// with their rows hidden until the plugin runs again.
fn requires_line(bundle: &WorkspaceBundle) -> Option<SharedString> {
    let missing: Vec<&str> = bundle
        .requires
        .iter()
        .filter(|id| !rox_services::plugins::is_running(id))
        .map(String::as_str)
        .collect();
    if missing.is_empty() {
        return None;
    }

    Some(rox_i18n::t!(
        "workspace-requires-missing",
        count = missing.len() as u64,
        plugins = missing.join(", ")
    ))
}

impl ApplyCard {
    /// A name that no longer resolves still gets a bare card.
    pub fn for_name(name: &str) -> ApplyCard {
        match resolve(name) {
            Some(bundle) => ApplyCard::of(&bundle),
            None => ApplyCard {
                name: name.to_string(),
                byline: None,
                description: None,
                shaders: Vec::new(),
                screen_shader: None,
                wears_shaders: false,
                requires: None,
                heavy: None,
            },
        }
    }

    pub fn of(bundle: &WorkspaceBundle) -> ApplyCard {
        let meta = &bundle.meta;
        let mut byline = Vec::new();
        if !meta.author.trim().is_empty() {
            byline.push(
                rox_i18n::t!("workspace-byline-author", author = meta.author.trim()).to_string(),
            );
        }
        if !meta.version.trim().is_empty() {
            byline.push(
                rox_i18n::t!("workspace-byline-version", version = meta.version.trim()).to_string(),
            );
        }
        ApplyCard {
            name: bundle.name.clone(),
            byline: (!byline.is_empty()).then(|| byline.join(", ").into()),
            description: display_blurb(&bundle.name, &meta.description),
            shaders: unapproved_shaders(bundle),
            screen_shader: screen_shader_line(bundle),
            wears_shaders: wears_shaders(bundle),
            requires: requires_line(bundle),
            heavy: heavy_line(bundle),
        }
    }

    pub fn shader_line(&self) -> Option<SharedString> {
        if self.shaders.is_empty() {
            return None;
        }
        let names: Vec<&str> = self
            .shaders
            .iter()
            .map(|shader| shader.label.as_str())
            .collect();
        Some(rox_i18n::t!(
            "workspace-apply-shader-count",
            count = self.shaders.len() as u64,
            names = names.join(", ")
        ))
    }

    /// Unapproved pool code splits it too: installing it is the moment to ask.
    pub fn splits_apply(&self) -> bool {
        self.wears_shaders || !self.shaders.is_empty()
    }

    /// Only ever called from the dialog's Approve button.
    pub fn approve_shaders(&self) {
        for shader in &self.shaders {
            shader::approve(&shader.source);
        }
    }
}

/// Re-link a freshly applied pool to the files its shaders were ejected
/// to, since a bundle is scrubbed of local bookmarks on the way out. The
/// file must still hash the same as the entry: never aim a reload at text
/// nobody approved. Returns whether anything re-linked.
pub(crate) fn relink_ejected(workspace: &str, pool: &mut [NamedShader]) -> bool {
    relink_ejected_in(&settings::shaders_dir(), workspace, pool)
}

fn relink_ejected_in(root: &Path, workspace: &str, pool: &mut [NamedShader]) -> bool {
    let mut linked = false;
    for entry in pool.iter_mut().filter(|entry| entry.path.is_none()) {
        let path = settings::shader_eject_path_in(root, workspace, &entry.name);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if shader::fingerprint(&text) != shader::fingerprint(&entry.source) {
            continue;
        }
        entry.path = Some(path);
        linked = true;
    }
    linked
}

pub fn all() -> Vec<Entry> {
    let mut list = shipped();
    list.extend(saved());
    list
}

pub fn path_for(name: &str) -> PathBuf {
    settings::workspaces_dir().join(file_name(name))
}

/// A name of pure punctuation falls back to "workspace".
fn file_name(name: &str) -> String {
    format!(
        "{}.{}",
        settings::safe_file_stem(name, "workspace"),
        roxw::EXT
    )
}

fn is_archive(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case(roxw::EXT))
}

/// Packs the bundle with its files from the live asset folder.
pub fn store(bundle: &WorkspaceBundle) -> bool {
    store_in(
        &settings::workspaces_dir(),
        bundle,
        Some(&asset_store::dir()),
    )
}

/// A file `assets_from` lacks, or holds with other bytes, comes out of the
/// archive already saved under the name. That's how a card edit on a
/// workspace that isn't the live one keeps its files.
fn store_in(dir: &Path, bundle: &WorkspaceBundle, assets_from: Option<&Path>) -> bool {
    let path = dir.join(file_name(&bundle.name));
    let files = Files {
        dir: assets_from,
        archives: vec![path.clone(), file_of_in(dir, &bundle.name)],
        ..Files::default()
    };

    match pack_in(&path, bundle, files) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("workspace: saving {:?}: {e}", bundle.name);
            false
        }
    }
}

/// Where [`pack_in`] looks for a table's files, matched by hash, in this
/// order.
#[derive(Default)]
struct Files<'a> {
    /// Bytes a version 1 bundle carried inline.
    drained: Drained,
    /// A folder holding them by file name.
    dir: Option<&'a Path>,
    /// Archives holding them under any name.
    archives: Vec<PathBuf>,
}

/// Write `bundle` to `path` as an archive. A bundle still carrying version
/// 1's inline images is upgraded on the way, its bytes packed as files.
fn pack_in(path: &Path, bundle: &WorkspaceBundle, mut files: Files) -> Result<(), String> {
    let mut bundle = bundle.clone();
    files.drained.extend(roxw::upgrade(&mut bundle));

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    note_own_write(path);

    // The common save: every file already sits in one folder. With an empty
    // table the folder is never read.
    let direct = files.drained.is_empty()
        && bundle
            .assets
            .iter()
            .all(|asset| files.dir.is_some_and(|from| holds(from, asset)));
    if direct {
        return roxw::write(path, &bundle, files.dir.unwrap_or(path));
    }

    let staging = suffixed(path, ".files");
    let _ = std::fs::remove_dir_all(&staging);
    let result =
        gather(&staging, &bundle, &files).and_then(|()| roxw::write(path, &bundle, &staging));
    let _ = std::fs::remove_dir_all(&staging);

    result
}

/// Each table file into `staging`, from the first source holding its bytes.
fn gather(staging: &Path, bundle: &WorkspaceBundle, files: &Files) -> Result<(), String> {
    let fail = |e: std::io::Error| format!("{}: {e}", staging.display());
    std::fs::create_dir_all(staging).map_err(fail)?;

    for asset in &bundle.assets {
        // The table can come from an imported file, so its names are
        // checked before one is joined onto a path.
        if !asset_store::flat_name(&asset.file) {
            return Err(format!("{} isn't a plain file name", asset.file));
        }
        let to = staging.join(&asset.file);

        if let Some((_, bytes)) = files
            .drained
            .iter()
            .find(|(held, _)| held.sha256 == asset.sha256)
        {
            std::fs::write(&to, bytes).map_err(fail)?;
            continue;
        }

        if let Some(from) = files.dir.filter(|from| holds(from, asset)) {
            std::fs::copy(from.join(&asset.file), &to).map_err(fail)?;
            continue;
        }

        let Some(bytes) = files
            .archives
            .iter()
            .find_map(|archive| from_archive(archive, asset))
        else {
            return Err(format!(
                "{} isn't in the asset folder or a saved workspace",
                asset.file
            ));
        };
        std::fs::write(&to, bytes).map_err(fail)?;
    }

    Ok(())
}

/// Whether `dir` holds the table entry's file with the bytes it names.
fn holds(dir: &Path, asset: &Asset) -> bool {
    asset_store::flat_name(&asset.file)
        && asset_store::sha256_file(&dir.join(&asset.file))
            .is_ok_and(|(bytes, sha256)| bytes == asset.bytes && sha256 == asset.sha256)
}

/// The bytes an archive holds for `asset`, found by hash and checked by
/// [`roxw::read_asset`].
fn from_archive(archive: &Path, asset: &Asset) -> Option<Vec<u8>> {
    let saved = roxw::read_bundle(archive).ok()?;
    let held = saved
        .assets
        .iter()
        .find(|held| held.sha256 == asset.sha256)?;

    roxw::read_asset(archive, &saved, &held.id).ok()
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Past the watch debounce, so a UI save's own event never comes back as a
/// reload. An outside edit inside the window is caught on its next save.
const OWN_WRITE_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);

static OWN_WRITES: std::sync::Mutex<Vec<(PathBuf, std::time::Instant)>> =
    std::sync::Mutex::new(Vec::new());

fn note_own_write(path: &Path) {
    let mut writes = OWN_WRITES.lock().unwrap();
    writes.retain(|(_, at)| at.elapsed() < OWN_WRITE_WINDOW);
    writes.push((path.to_path_buf(), std::time::Instant::now()));
}

fn was_own_write(path: &Path) -> bool {
    OWN_WRITES
        .lock()
        .unwrap()
        .iter()
        .any(|(p, at)| p == path && at.elapsed() < OWN_WRITE_WINDOW)
}

/// A missing file is a success: the list can be one external delete stale.
pub fn remove(name: &str) {
    remove_in(&settings::workspaces_dir(), name);
}

fn remove_in(dir: &Path, name: &str) {
    let path = file_of_in(dir, name);
    if let Err(e) = std::fs::remove_file(&path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!("workspace: deleting {}: {e}", path.display());
    }
}

/// Only the migration calls this. It tells a replay (a crash before the
/// first save) from two names that fold to one filename.
pub(crate) fn migrate_saved(bundle: WorkspaceBundle) {
    migrate_saved_in(&settings::workspaces_dir(), bundle);
}

fn migrate_saved_in(dir: &Path, mut bundle: WorkspaceBundle) {
    let path = dir.join(file_name(&bundle.name));
    match read_file(&path) {
        Some(existing) if existing.name == bundle.name => return,
        // A different workspace holds the file its name folds to, so this one
        // takes a file and name of its own.
        Some(existing) => {
            let taken: Vec<String> = saved_in(dir).into_iter().map(|entry| entry.name).collect();
            let stem = stem_of(&path).unwrap_or_else(|| bundle.name.clone());
            let renamed = unique_name(&stem, |candidate| taken.iter().any(|n| n == candidate));
            log::info!(
                "settings: workspace {:?} shares a filename with {:?}, saving it as {renamed:?}",
                bundle.name,
                existing.name
            );
            bundle.name = renamed;
        }
        None => {}
    }
    store_in(dir, &bundle, None);
}

/// Apply's first step: the incoming workspace's files staged into
/// `<data>/assets.incoming/`, then swapped in whole for the live folder and
/// table. A failure leaves both as they were, so the caller stops before
/// anything else is replaced.
pub(crate) fn swap_in_files(bundle: &WorkspaceBundle, origin: &Origin) -> Result<(), String> {
    let staged = suffixed(&asset_store::dir(), ".incoming");
    swap_in_files_with(&staged, bundle, origin, asset_store::swap_in)
}

fn swap_in_files_with(
    staged: &Path,
    bundle: &WorkspaceBundle,
    origin: &Origin,
    swap: impl FnOnce(&Path, Vec<Asset>) -> Result<(), String>,
) -> Result<(), String> {
    let result =
        stage_in(staged, bundle, origin).and_then(|()| swap(staged, bundle.assets.clone()));
    if result.is_err() {
        let _ = std::fs::remove_dir_all(staged);
    }

    result
}

fn stage_in(staged: &Path, bundle: &WorkspaceBundle, origin: &Origin) -> Result<(), String> {
    // An apply that died part way leaves its stage behind.
    let _ = std::fs::remove_dir_all(staged);

    match origin {
        Origin::Saved(archive) => roxw::extract_assets(archive, bundle, staged),
        Origin::Shipped(stem) => {
            let fail = |e: std::io::Error| format!("{}: {e}", staged.display());
            std::fs::create_dir_all(staged).map_err(fail)?;

            for asset in &bundle.assets {
                let bytes = shipped_asset(stem, asset)?;
                std::fs::write(staged.join(&asset.file), bytes).map_err(fail)?;
            }

            Ok(())
        }
    }
}

/// A shipped workspace's file, held to its table entry the way an archive's
/// is, so a JSON edited out of step with its folder fails the apply.
fn shipped_asset(stem: &str, asset: &Asset) -> Result<Cow<'static, [u8]>, String> {
    if !asset_store::flat_name(&asset.file) {
        return Err(format!("{} isn't a plain file name", asset.file));
    }

    let bytes = assets::shipped_workspace_asset(stem, &asset.file)
        .ok_or_else(|| format!("{stem} ships no {}", asset.file))?;
    if bytes.len() as u64 != asset.bytes || asset_store::sha256_hex(&bytes) != asset.sha256 {
        return Err(format!("{} doesn't match its hash", asset.file));
    }

    Ok(bytes)
}

/// A saved workspace's archive copied out byte for byte, a shipped one packed
/// from the files it embeds. Returns the size written.
pub(crate) fn export_to(name: &str, origin: &Origin, to: &Path) -> Result<u64, String> {
    let fail = |e: std::io::Error| format!("{}: {e}", to.display());

    match origin {
        Origin::Saved(archive) => {
            // A copy onto itself truncates the source before reading it.
            let same = std::fs::canonicalize(archive)
                .ok()
                .zip(std::fs::canonicalize(to).ok())
                .is_some_and(|(from, to)| from == to);
            if !same {
                std::fs::copy(archive, to).map_err(fail)?;
            }
        }
        Origin::Shipped(_) => {
            let (bundle, _) =
                shipped_bundle(name).ok_or_else(|| format!("{name} isn't shipped"))?;
            let staging = suffixed(to, ".files");
            let result = stage_in(&staging, &bundle, origin)
                .and_then(|()| roxw::write(to, &bundle, &staging));
            let _ = std::fs::remove_dir_all(&staging);
            result?;
        }
    }

    std::fs::metadata(to).map(|meta| meta.len()).map_err(fail)
}

/// Persist the bundle in one write and push each appearance knob live. The
/// layout swap and mini-player roles are the caller's, since they need the
/// workspace whose dock they change. The files go first, through
/// [`swap_in_files`].
pub fn apply_look(bundle: &WorkspaceBundle, cx: &mut App) {
    // Persisted up front; the live statics below only repaint.
    let persist = bundle.clone();
    Settings::update(move |s| persist.apply_to(s));
    show_look(bundle, cx);
}

/// The live half of [`apply_look`], for a bundle already on disk.
fn show_look(bundle: &WorkspaceBundle, cx: &mut App) {
    // Before the app font is set: gpui remembers a family it couldn't find
    // for the rest of the session.
    crate::workspace_fonts::register(cx);

    palette::set_palettes(
        Palette::from_map(&bundle.palette_dark),
        Palette::from_map_over(Palette::light(), &bundle.palette_light),
        cx,
    );
    let a = &bundle.appearance;
    palette::set_scalars(a.surface_opacity, a.backdrop_strength, cx);
    palette::set_backdrop_all_windows(a.backdrop_all_windows, cx);
    settings::set_app_frame(a.frame, cx);
    settings::set_seams(a.seams, cx);
    palette::set_keep_theme(a.keep_theme, cx);
    palette::set_art_theming(a.art_theming, cx);
    settings::set_app_font(a.app_font.clone(), cx);
    settings::set_rating_style(a.rating_style, cx);
    settings::set_rating_dots(a.rating_dots, cx);
    tokens::set_slider_look(a.slider, cx);
    settings::set_hide_menubar(a.hide_menubar, cx);
    settings::set_menubar_buttons(a.menubar_buttons, cx);
    settings::set_os_decorations(a.os_decorations);
    settings::set_bare_child_windows(a.bare_child_windows);
    settings::set_child_titlebar(a.child_titlebar);
    settings::set_chrome_style(a.chrome_style);
    settings::set_chrome_side(a.chrome_side);
    settings::set_resize_border(a.resize_border);
    settings::set_backdrop_visual_look(&a.milkdrop);
    crate::backdrop_visual::wake(cx);
    crate::workspace::apply_decorations(cx);
    crate::workspace::apply_resize_border(cx);
}

/// The file the list matched, so a hand-dropped file keeps its filename.
fn file_of_in(dir: &Path, name: &str) -> PathBuf {
    saved_in(dir)
        .into_iter()
        .find(|entry| entry.name == name)
        .and_then(|entry| entry.path)
        .unwrap_or_else(|| dir.join(file_name(name)))
}

/// The user's own first, so a saved bundle shadows a shipped one.
pub fn resolve(name: &str) -> Option<WorkspaceBundle> {
    resolve_with_origin(name).map(|(bundle, _)| bundle)
}

/// [`resolve`], with where the workspace's files are.
pub fn resolve_with_origin(name: &str) -> Option<(WorkspaceBundle, Origin)> {
    resolve_in(&settings::workspaces_dir(), name)
}

fn resolve_in(dir: &Path, name: &str) -> Option<(WorkspaceBundle, Origin)> {
    let path = file_of_in(dir, name);
    if let Some(bundle) = read_file(&path) {
        return Some((bundle, Origin::Saved(path)));
    }

    shipped_bundle(name).map(|(bundle, stem)| (bundle, Origin::Shipped(stem)))
}

pub(crate) fn unique_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} ({n})"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| base.to_string())
}

/// Read a `.roxw` or a version 1 `.json`, dedupe its name, store it. The
/// error is the reason a file was refused, fit for a toast or a socket
/// answer.
pub fn import_file(path: &Path) -> Result<WorkspaceBundle, String> {
    import_file_in(&settings::workspaces_dir(), path)
}

/// Deduped against the current workspaces, so an import never shadows one.
/// The stored copy is always a fresh archive, so whatever an imported one
/// held past its manifest and table stays behind.
fn import_file_in(dir: &Path, path: &Path) -> Result<WorkspaceBundle, String> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    let (mut bundle, files) = match ext.as_deref() {
        Some(roxw::EXT) => (
            roxw::read_bundle(path)?,
            Files {
                archives: vec![path.to_path_buf()],
                ..Files::default()
            },
        ),
        Some("json") => {
            let (bundle, drained) = roxw::read_v1(path)?;
            (
                bundle,
                Files {
                    drained,
                    ..Files::default()
                },
            )
        }
        _ => return Err(format!("{} isn't a workspace file", path.display())),
    };

    if bundle.name.trim().is_empty() {
        bundle.name = stem_of(path).unwrap_or_else(|| "imported".to_string());
    }
    // A name whose file is taken would overwrite another workspace's
    // archive, so the file counts as well as the listed names.
    let taken: Vec<String> = shipped()
        .into_iter()
        .chain(saved_in(dir))
        .map(|entry| entry.name)
        .collect();
    bundle.name = unique_name(&bundle.name, |candidate| {
        taken.iter().any(|name| name == candidate) || dir.join(file_name(candidate)).exists()
    });

    pack_in(&dir.join(file_name(&bundle.name)), &bundle, files)?;

    Ok(bundle)
}

/// Where the folder migration puts the JSON files it converted.
const V1_BACKUP: &str = "v1-backup";

/// Version 1's saved workspaces, `workspaces/*.json`, into archives. Each
/// JSON moves to `workspaces/v1-backup/` once its archive is written, so a
/// second run finds nothing to convert.
pub(crate) fn migrate_v1_files() {
    migrate_v1_files_in(&settings::workspaces_dir());
}

fn migrate_v1_files_in(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        })
        .collect();
    // Sorted, so which side of a collision keeps the name doesn't hang on
    // the order the folder lists in.
    found.sort();

    for json in found {
        match migrate_v1_file(dir, &json) {
            Ok(name) => log::info!("workspace: converted {} to {name:?}", json.display()),
            Err(e) => log::warn!("workspace: converting {}: {e}", json.display()),
        }
    }
}

fn migrate_v1_file(dir: &Path, json: &Path) -> Result<String, String> {
    let stem = stem_of(json).ok_or_else(|| "the file has no usable name".to_string())?;
    let (mut bundle, drained) = roxw::read_v1(json)?;
    if bundle.name.trim().is_empty() {
        bundle.name = stem.clone();
    }

    let target = dir.join(format!("{stem}.{}", roxw::EXT));
    let target = match read_file(&target) {
        None if !target.exists() => target,
        // A replay: the archive was written and the move never happened.
        Some(existing) if same_bundle(&existing, &bundle) => {
            back_up(dir, json)?;
            return Ok(existing.name);
        }
        // A different workspace holds the archive this one's file maps to,
        // so this one takes a file and name of its own.
        _ => {
            let taken: Vec<String> = saved_in(dir).into_iter().map(|entry| entry.name).collect();
            let renamed = unique_name(&stem, |candidate| {
                taken.iter().any(|name| name == candidate)
                    || dir.join(format!("{candidate}.{}", roxw::EXT)).exists()
            });
            bundle.name = renamed.clone();
            dir.join(format!("{renamed}.{}", roxw::EXT))
        }
    };

    pack_in(
        &target,
        &bundle,
        Files {
            drained,
            ..Files::default()
        },
    )?;
    back_up(dir, json)?;

    Ok(bundle.name)
}

/// Compared as written, since a converted bundle and the archive it became
/// read back through the same upgrade.
fn same_bundle(a: &WorkspaceBundle, b: &WorkspaceBundle) -> bool {
    serde_json::to_value(a).ok() == serde_json::to_value(b).ok()
}

/// Moved, never deleted. A name already backed up gets a numbered one.
fn back_up(dir: &Path, json: &Path) -> Result<(), String> {
    let backup = dir.join(V1_BACKUP);
    std::fs::create_dir_all(&backup).map_err(|e| format!("{}: {e}", backup.display()))?;

    let stem = stem_of(json).unwrap_or_else(|| "workspace".to_string());
    let free = unique_name(&stem, |candidate| {
        backup.join(format!("{candidate}.json")).exists()
    });
    let to = backup.join(format!("{free}.json"));

    std::fs::rename(json, &to).map_err(|e| format!("{}: {e}", to.display()))
}

const WATCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(500);

/// Re-apply the active workspace when its file changes on disk (ADR 22).
/// A file [`store`] wrote is skipped, so a UI save never reloads itself.
pub(crate) fn watch(cx: &mut App) {
    use notify_debouncer_full::notify::{EventKind, RecursiveMode};
    use notify_debouncer_full::{DebounceEventResult, new_debouncer};

    let dir = settings::workspaces_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        log::warn!("workspace watch: creating {}: {e}", dir.display());
        return;
    }
    let (tx, events) = async_channel::unbounded::<Vec<PathBuf>>();
    let mut debouncer =
        match new_debouncer(WATCH_DEBOUNCE, None, move |result: DebounceEventResult| {
            // On the debouncer's thread. Only writes matter: deleting the active file
            // leaves the live look standing.
            let Ok(batch) = result else { return };
            let paths: Vec<PathBuf> = batch
                .iter()
                .filter(|event| matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)))
                .flat_map(|event| event.paths.iter().cloned())
                .filter(|path| is_archive(path))
                .collect();
            if !paths.is_empty() {
                let _ = tx.send_blocking(paths);
            }
        }) {
            Ok(debouncer) => debouncer,
            Err(e) => {
                log::warn!("workspace watch: not watching: {e}");
                return;
            }
        };
    if let Err(e) = debouncer.watch(&dir, RecursiveMode::NonRecursive) {
        log::warn!("workspace watch: not watching {}: {e}", dir.display());
        return;
    }
    log::info!("workspace watch: watching {}", dir.display());
    cx.spawn(async move |cx| {
        let _hold = debouncer;
        while let Ok(paths) = events.recv().await {
            if cx.update(|cx| reload_if_active(&paths, cx)).is_err() {
                break;
            }
        }
    })
    .detach();
}

/// A file that no longer reads leaves the current look standing.
fn reload_if_active(paths: &[PathBuf], cx: &mut App) {
    let active = Settings::load().look.bundle.name;
    if active.trim().is_empty() {
        return;
    }
    let file = file_of_in(&settings::workspaces_dir(), &active);
    if !paths.contains(&file) {
        return;
    }
    if was_own_write(&file) {
        return;
    }
    match roxw::read_bundle(&file) {
        Ok(_) => {
            log::info!("workspace: {active:?} changed on disk, re-applying");
            crate::workspace::apply_workspace_to_front(&active, cx);
        }
        Err(e) => log::warn!(
            "workspace: {} changed on disk but doesn't read, keeping the current look: {e}",
            file.display()
        ),
    }
}

/// What the live look watch makes of `workspace.json`'s text.
enum LiveChange {
    /// rox wrote it.
    Own(Box<LookState>),
    Edited(Box<LookState>),
    /// Half-saved, or not a look at all. The live look stays.
    Unreadable(String),
}

fn read_live(text: &str, own: impl Fn(&str) -> bool) -> LiveChange {
    let look: LookState = match serde_json::from_str(text) {
        Ok(look) => look,
        Err(e) => return LiveChange::Unreadable(e.to_string()),
    };

    if own(text) {
        return LiveChange::Own(Box::new(look));
    }

    if look.bundle.version > WORKSPACE_VERSION {
        return LiveChange::Unreadable(format!(
            "the look is version {}, newer than this build reads ({WORKSPACE_VERSION})",
            look.bundle.version
        ));
    }

    LiveChange::Edited(Box::new(look))
}

/// Each table entry whose file isn't in `dir` with the bytes it names. The
/// look still applies around them, the way a missing image degrades.
fn asset_problems(dir: &Path, table: &[Asset]) -> Vec<String> {
    table
        .iter()
        .filter_map(|asset| {
            if !asset_store::flat_name(&asset.file) {
                return Some(format!("{:?} isn't a plain file name", asset.file));
            }

            match asset_store::sha256_file(&dir.join(&asset.file)) {
                Ok((bytes, sha256)) if bytes == asset.bytes && sha256 == asset.sha256 => None,
                Ok(_) => Some(format!("{} doesn't match its hash", asset.file)),
                Err(e) => Some(e),
            }
        })
        .collect()
}

/// The last dock dump the watch saw in the file. A hand edit rebuilds the
/// dock only when this moved, since the dump rox writes back after a
/// rebuild never reads identical to the one typed in.
struct LiveSeen {
    layout: Option<serde_json::Value>,
}

/// Re-apply the live look when `workspace.json` changes from outside rox
/// (ADR 22). Its folder is watched, since an editor that saves by rename
/// replaces the file a direct watch would hold.
pub(crate) fn watch_live(cx: &mut App) {
    use notify_debouncer_full::notify::{EventKind, RecursiveMode};
    use notify_debouncer_full::{DebounceEventResult, new_debouncer};

    let file = settings::look_path();
    let Some(dir) = file.parent().map(Path::to_path_buf) else {
        return;
    };
    let (tx, events) = async_channel::unbounded::<()>();

    let watched = file.clone();
    let mut debouncer =
        match new_debouncer(WATCH_DEBOUNCE, None, move |result: DebounceEventResult| {
            let Ok(batch) = result else { return };
            let touched = batch
                .iter()
                .filter(|event| matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)))
                .any(|event| event.paths.contains(&watched));
            if touched {
                let _ = tx.send_blocking(());
            }
        }) {
            Ok(debouncer) => debouncer,
            Err(e) => {
                log::warn!("look watch: not watching: {e}");
                return;
            }
        };
    if let Err(e) = debouncer.watch(&dir, RecursiveMode::NonRecursive) {
        log::warn!("look watch: not watching {}: {e}", dir.display());
        return;
    }
    log::info!("look watch: watching {}", file.display());

    let mut seen = LiveSeen {
        layout: Settings::load().look.layout,
    };
    cx.spawn(async move |cx| {
        let _hold = debouncer;
        while let Ok(()) = events.recv().await {
            if cx.update(|cx| reload_live(&file, &mut seen, cx)).is_err() {
                break;
            }
        }
    })
    .detach();
}

fn reload_live(file: &Path, seen: &mut LiveSeen, cx: &mut App) {
    // Gone is the same as unreadable: the live look stays standing.
    let Ok(text) = std::fs::read_to_string(file) else {
        return;
    };

    let edited = match read_live(&text, settings::live_look::is_own_write) {
        LiveChange::Own(look) => {
            seen.layout = look.layout;
            return;
        }
        LiveChange::Unreadable(e) => {
            log::warn!(
                "look: {} changed on disk but doesn't read, keeping the current look: {e}",
                file.display()
            );
            return;
        }
        LiveChange::Edited(look) => look,
    };
    log::info!("look: {} changed on disk, re-applying", file.display());
    settings::live_look::forget_own_writes();

    // Through the load, which clamps what a hand edit can push out of range.
    // Everything here is already on disk, so nothing below writes the look
    // back unless a window rebuilds its dock.
    let look = Settings::load().look;
    let dock_moved = edited.layout != seen.layout;
    seen.layout = edited.layout;

    for problem in asset_problems(&asset_store::dir(), &look.bundle.assets) {
        log::warn!("look: {problem}");
    }
    asset_store::note(look.bundle.assets.clone());

    show_look(&look.bundle, cx);
    settings::note_shader_pool(look.bundle.shaders.clone());
    settings::note_backdrop_shader(look.bundle.backdrop_shader.clone());
    crate::workspace::reload_live_look_in_front(&look, dock_moved, cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn named_bundle(name: &str) -> WorkspaceBundle {
        WorkspaceBundle {
            name: name.into(),
            ..Default::default()
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rox-ws-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The filter drops an unparseable bundle silently, so a typo would just
    /// vanish from the list.
    #[test]
    fn every_shipped_asset_parses() {
        let files = rox_design::assets::shipped_workspaces();
        assert!(!files.is_empty());
        let parsed = shipped();
        assert_eq!(
            parsed.len(),
            files.len(),
            "a shipped workspace file failed to parse: {:?} vs {:?}",
            files.iter().map(|(stem, _)| stem).collect::<Vec<_>>(),
            parsed.iter().map(|e| &e.name).collect::<Vec<_>>()
        );
    }

    /// A panel's `info` is opaque to the bundle parse, so a bad piece name would
    /// ship as a panel quietly stuck in its stock arrangement. Track info is the
    /// one the bundles write by hand.
    #[test]
    fn every_shipped_track_info_config_reads() {
        fn walk(node: &serde_json::Value, stem: &str) {
            if node.get("panel_name").and_then(|name| name.as_str()) == Some("track info") {
                let info = &node["info"]["panel"];
                let read: Result<rox_panels::transport::TrackInfoConfig, _> =
                    serde_json::from_value(info.clone());
                assert!(read.is_ok(), "{stem}: {info}");
            }
            for child in node
                .get("children")
                .and_then(|kids| kids.as_array())
                .into_iter()
                .flatten()
            {
                walk(child, stem);
            }
        }
        for (stem, bytes) in rox_design::assets::shipped_workspaces() {
            let bundle: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            for layout in bundle["layouts"].as_array().into_iter().flatten() {
                walk(&layout["dump"]["center"], &stem);
            }
        }
    }

    /// Old layouts load through the legacy fold, but a shipped bundle should
    /// already be in the current shape: no retired mini toggle panel, and
    /// every window controls panel holding its own item list. Walks every
    /// node, docks and composite children included.
    #[test]
    fn every_shipped_window_controls_is_current() {
        fn walk(node: &serde_json::Value, stem: &str) {
            match node {
                serde_json::Value::Object(map) => {
                    let name = map.get("panel_name").and_then(|name| name.as_str());
                    assert_ne!(name, Some("mini toggle"), "{stem}: a retired mini toggle");
                    if name == Some("window controls") {
                        let info = &node["info"]["panel"];
                        assert!(info.get("items").is_some(), "{stem}: legacy shape {info}");
                        let read: Result<crate::panels::window_controls::WindowControlsConfig, _> =
                            serde_json::from_value(info.clone());
                        assert!(read.is_ok(), "{stem}: {info}");
                    }
                    map.values().for_each(|value| walk(value, stem));
                }
                serde_json::Value::Array(values) => {
                    values.iter().for_each(|value| walk(value, stem));
                }
                _ => {}
            }
        }
        for (stem, bytes) in rox_design::assets::shipped_workspaces() {
            let bundle: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            walk(&bundle, &stem);
        }
    }

    /// The splitter and assets only run at registration, so without this a
    /// mistyped `// @pass` or a mangled plate ships as a blank panel. The WGSL
    /// itself is naga's gate.
    #[test]
    fn every_shipped_shader_splits_and_finds_its_images() {
        for (stem, bytes) in assets::shipped_workspaces() {
            let bundle: WorkspaceBundle =
                serde_json::from_slice(&bytes).unwrap_or_else(|err| panic!("{stem}: {err}"));
            assert_eq!(
                bundle.version, WORKSPACE_VERSION,
                "{stem}: a shipped bundle is the current version"
            );
            for pool in &bundle.shaders {
                let where_ = format!("{stem}: shader '{}'", pool.name);
                let spec = shader::parse_chain(&pool.source)
                    .unwrap_or_else(|err| panic!("{where_}: {err}"));
                assert!(
                    !spec.passes.is_empty(),
                    "{where_}: a program needs at least one pass"
                );
                for asset in &spec.assets {
                    // The cover binding has no bytes; the player supplies it.
                    if asset.is_cover() {
                        continue;
                    }
                    let carried = pool
                        .assets
                        .iter()
                        .find(|held| held.file == asset.file)
                        .unwrap_or_else(|| {
                            panic!("{where_}: declares {} and doesn't carry it", asset.file)
                        });
                    assert!(
                        carried.data.is_empty(),
                        "{where_}: {} is inline",
                        asset.file
                    );
                    let entry = bundle
                        .assets
                        .iter()
                        .find(|entry| entry.id == carried.asset)
                        .unwrap_or_else(|| {
                            panic!("{where_}: {} names no asset {}", asset.file, carried.asset)
                        });
                    let bytes = shipped_asset(&stem, entry)
                        .unwrap_or_else(|err| panic!("{where_}: {}: {err}", asset.file));
                    let image = image::load_from_memory(&bytes).unwrap_or_else(|err| {
                        panic!("{where_}: {} won't decode: {err}", asset.file)
                    });
                    assert!(
                        image.width() > 0 && image.height() > 0,
                        "{where_}: {} decoded to nothing",
                        asset.file
                    );
                }
            }
            // A shipped screen shader has to be an overlay, or it covers the app the
            // moment someone applies a look they didn't pick the shader for.
            let Some(post) = &bundle.post_shader else {
                continue;
            };
            let source = match post.name.as_deref() {
                Some(name) => bundle
                    .shaders
                    .iter()
                    .find(|entry| entry.name == name)
                    .map(|entry| entry.source.clone())
                    .unwrap_or_else(|| panic!("{stem}: overlay shader '{name}' isn't in the pool")),
                None => post.source.clone(),
            };
            if source.trim().is_empty() {
                continue;
            }
            assert!(
                shader::overlay(&source),
                "{stem}: the overlay shader covers the window; it needs `// @overlay` \
                 (and to actually leave the app readable)"
            );
        }
    }

    #[test]
    fn slugs_match_the_keys_the_locales_carry() {
        assert_eq!(shipped_slug("(Default)"), "default");
        assert_eq!(shipped_slug("Llama (WinAmp)"), "llama-winamp");
        assert_eq!(shipped_slug("CaTRoX"), "catrox");
        assert_eq!(shipped_slug("Foobar"), "foobar");
    }

    #[test]
    fn only_the_word_is_translated() {
        assert_eq!(display_title("Foobar"), "Foobar");
        assert_eq!(display_title("Phosphor"), "Phosphor");
        assert_eq!(display_title("Someone Else's Look"), "Someone Else's Look");
        assert_eq!(
            display_title("(Default)"),
            rox_i18n::t!("workspace-shipped-default")
        );
    }

    #[test]
    fn every_shipped_bundle_has_a_translated_blurb() {
        for entry in shipped() {
            let key = format!("workspace-shipped-{}-blurb", shipped_slug(&entry.name));
            assert!(
                rox_i18n::try_translate(&key).is_some(),
                "{} ships without {key}",
                entry.name
            );
        }
    }

    #[test]
    fn shipped_trust_collects_every_shader_a_bundle_carries() {
        let bundle = WorkspaceBundle {
            shaders: vec![NamedShader {
                name: "Grain".into(),
                source: "// the pool entry".into(),
                path: None,
                assets: Vec::new(),
            }],
            post_shader: Some(rox_core::settings::PostShaderConfig {
                enabled: true,
                source: "// the screen one".into(),
                ..Default::default()
            }),
            layouts: vec![rox_core::settings::NamedLayout {
                name: "one".into(),
                size: None,
                dump: serde_json::json!({
                    "panel_name": "StackPanel",
                    "children": [
                        {
                            "panel_name": "shader",
                            "info": { "panel": { "source": "// the shader panel" }},
                        },
                        {
                            "panel_name": "waveform",
                            "info": { "panel": {
                                "shader": { "source": "// the surface one" },
                            }},
                        },
                        { "panel_name": "queue", "info": { "panel": { "source": "" }}},
                    ],
                }),
            }],
            ..WorkspaceBundle::default()
        };

        let prints = bundle_fingerprints(&bundle);
        let expected: Vec<String> = [
            "// the pool entry",
            "// the screen one",
            "// the shader panel",
            "// the surface one",
        ]
        .into_iter()
        .map(shader::fingerprint)
        .collect();
        assert_eq!(prints.len(), expected.len(), "{prints:?}");
        for print in expected {
            assert!(prints.contains(&print), "{prints:?}");
        }

        // Also proves the seeding never panics on what the build ships.
        trust_shipped_shaders();
    }

    #[test]
    fn the_review_lists_each_unapproved_shader_once() {
        let agreed = "// this one is already agreed to";
        let bundle = WorkspaceBundle {
            shaders: vec![
                NamedShader {
                    name: "Grain".into(),
                    source: "// grain".into(),
                    path: None,
                    assets: Vec::new(),
                },
                NamedShader {
                    name: "Bloom".into(),
                    source: "// bloom".into(),
                    path: None,
                    assets: Vec::new(),
                },
                NamedShader {
                    name: "Old".into(),
                    source: agreed.into(),
                    path: None,
                    assets: Vec::new(),
                },
            ],
            post_shader: Some(rox_core::settings::PostShaderConfig {
                enabled: true,
                name: Some("Grain".into()),
                source: "// a stale copy of grain".into(),
                ..Default::default()
            }),
            layouts: vec![rox_core::settings::NamedLayout {
                name: "one".into(),
                size: None,
                dump: serde_json::json!({
                    "panel_name": "StackPanel",
                    "children": [
                        {
                            "panel_name": "waveform",
                            "info": { "panel": { "shader": { "source": "// grain" }}},
                        },
                        {
                            "panel_name": "shader",
                            "info": { "panel": { "source": "// only in the dump" }},
                        },
                        { "panel_name": "queue", "info": { "panel": { "source": "" }}},
                    ],
                }),
            }],
            ..WorkspaceBundle::default()
        };

        settings::note_approved(&shader::fingerprint(agreed));
        let pending = unapproved_shaders(&bundle);
        let labels: Vec<&str> = pending.iter().map(|s| s.label.as_str()).collect();
        let hashed = shader::fingerprint("// only in the dump")[..8].to_string();
        assert_eq!(labels, ["Grain", "Bloom", hashed.as_str()], "{labels:?}");

        let line = ApplyCard::of(&bundle).shader_line().expect("a shader line");
        let expected_prefix = rox_i18n::t!(
            "workspace-apply-shader-count",
            count = 3u64,
            names = "Grain, Bloom, "
        );
        assert!(line.starts_with(expected_prefix.as_ref()), "{line}");

        for shader in &pending {
            settings::note_approved(&shader::fingerprint(&shader.source));
        }
        assert!(unapproved_shaders(&bundle).is_empty());
        assert!(ApplyCard::of(&bundle).shader_line().is_none());

        for source in ["// grain", "// bloom", "// only in the dump", agreed] {
            settings::forget_approved(&shader::fingerprint(source));
        }
        assert!(unapproved_shaders(&WorkspaceBundle::default()).is_empty());
    }

    /// The with-or-without choice is about the look, not trust, so an agreed
    /// look still says it wears shaders.
    #[test]
    fn an_agreed_look_still_says_it_wears_shaders() {
        let source = "// worn everywhere";
        let bundle = WorkspaceBundle {
            shaders: vec![NamedShader {
                name: "Lace".into(),
                source: source.into(),
                path: None,
                assets: Vec::new(),
            }],
            post_shader: Some(rox_core::settings::PostShaderConfig {
                enabled: true,
                name: Some("Lace".into()),
                ..Default::default()
            }),
            layouts: vec![rox_core::settings::NamedLayout {
                name: "one".into(),
                size: None,
                dump: serde_json::json!({
                    "panel_name": "waveform",
                    "info": { "panel": { "shader": { "name": "Lace" }}},
                }),
            }],
            ..WorkspaceBundle::default()
        };

        settings::note_approved(&shader::fingerprint(source));
        let card = ApplyCard::of(&bundle);
        assert!(card.shaders.is_empty(), "nothing left to agree to");
        assert!(card.wears_shaders);
        assert!(card.splits_apply(), "the dialog still offers both yeses");
        settings::forget_approved(&shader::fingerprint(source));

        assert!(!ApplyCard::of(&WorkspaceBundle::default()).splits_apply());
    }

    #[test]
    fn applying_without_shaders_parks_them_rather_than_dropping_them() {
        let bundle = WorkspaceBundle {
            shaders: vec![NamedShader {
                name: "Lace".into(),
                source: "// lace".into(),
                path: None,
                assets: Vec::new(),
            }],
            post_shader: Some(rox_core::settings::PostShaderConfig {
                enabled: true,
                name: Some("Lace".into()),
                ..Default::default()
            }),
            layouts: vec![rox_core::settings::NamedLayout {
                name: "one".into(),
                size: None,
                dump: serde_json::json!({
                    "panel_name": "StackPanel",
                    "children": [
                        {
                            "panel_name": "waveform",
                            "info": { "panel": { "shader": { "name": "Lace" }}},
                        },
                        {
                            "panel_name": "shader",
                            "info": { "panel": { "source": "// its own" }},
                        },
                    ],
                }),
            }],
            panel_presets: vec![rox_core::settings::PanelPreset {
                name: "Scope".into(),
                panel: serde_json::json!({
                    "panel_name": "spectrum",
                    "info": { "panel": { "shader": { "source": "// a preset's own" }}},
                }),
            }],
            ..WorkspaceBundle::default()
        };

        assert!(
            unapproved_shaders(&bundle)
                .iter()
                .any(|pending| pending.source == "// a preset's own")
        );

        let bare = without_shaders(&bundle);
        assert!(!wears_shaders(&bare));
        let preset = &bare.panel_presets[0].panel["info"]["panel"]["shader"];
        assert_eq!(preset["enabled"], false);
        assert_eq!(preset["source"], "// a preset's own");
        let post = bare.post_shader.as_ref().expect("the overlay travels");
        assert!(!post.enabled);
        assert_eq!(post.name.as_deref(), Some("Lace"));
        assert_eq!(bare.shaders.len(), 1, "the pool travels either way");
        assert_eq!(bare.shaders[0].source, "// lace");
        assert_eq!(bare.layouts.len(), 1);
        let panels = bare.layouts[0].dump["children"]
            .as_array()
            .expect("children");
        assert_eq!(panels.len(), 2);
        let worn = &panels[0]["info"]["panel"]["shader"];
        assert_eq!(worn["enabled"], false);
        assert_eq!(worn["name"], "Lace", "the panel still knows what it wore");
        let own = &panels[1]["info"]["panel"];
        assert_eq!(own["enabled"], false);
        assert_eq!(own["source"], "// its own");
        assert!(wears_shaders(&bundle), "the original is left alone");
    }

    /// Approval is over code, and a plate isn't code (ADR 23).
    #[test]
    fn assets_are_data_and_never_ask_for_approval() {
        let source = "// @asset plate: plate.png";
        let bundle = |assets: Vec<rox_core::settings::ShaderAsset>| WorkspaceBundle {
            shaders: vec![NamedShader {
                name: "Serpent".into(),
                source: source.into(),
                path: None,
                assets,
            }],
            ..WorkspaceBundle::default()
        };

        settings::note_approved(&shader::fingerprint(source));
        let bare = bundle(Vec::new());
        let plated = bundle(vec![rox_core::settings::ShaderAsset {
            file: "plate.png".into(),
            asset: "serpent-plate".into(),
            data: String::new(),
        }]);
        assert!(unapproved_shaders(&plated).is_empty(), "a plate isn't code");
        assert_eq!(
            bundle_fingerprints(&bare),
            bundle_fingerprints(&plated),
            "the same source hashes the same however it's dressed"
        );

        settings::forget_approved(&shader::fingerprint(source));
        let pending = unapproved_shaders(&plated);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].label, "Serpent");
    }

    #[test]
    fn a_card_reads_out_as_a_byline() {
        let mut bundle = named_bundle("Nightfall");
        bundle.meta.author = "Nova".into();
        bundle.meta.version = "2.1".into();
        bundle.meta.description = "Warm and quiet.".into();
        let card = ApplyCard::of(&bundle);
        assert_eq!(card.name, "Nightfall");
        let expected_byline = format!(
            "{}, {}",
            rox_i18n::t!("workspace-byline-author", author = "Nova"),
            rox_i18n::t!("workspace-byline-version", version = "2.1")
        );
        assert_eq!(
            card.byline.as_ref().map(|line| line.as_ref()),
            Some(expected_byline.as_str())
        );
        assert_eq!(
            card.description.as_ref().map(|line| line.as_ref()),
            Some("Warm and quiet.")
        );

        let plain = ApplyCard::of(&named_bundle("Plain"));
        assert!(plain.byline.is_none());
        assert!(plain.description.is_none());
    }

    #[test]
    fn the_confirm_names_the_screen_shader_a_look_wears() {
        let mut bundle = named_bundle("Inked");
        bundle.shaders = vec![NamedShader {
            name: "Dither".into(),
            source: "// dither".into(),
            path: None,
            assets: Vec::new(),
        }];
        bundle.post_shader = Some(rox_core::settings::PostShaderConfig {
            enabled: true,
            name: Some("Dither".into()),
            ..Default::default()
        });
        let line = ApplyCard::of(&bundle).screen_shader.expect("a shader line");
        assert!(line.contains("Dither"), "{line}");

        bundle.post_shader = Some(rox_core::settings::PostShaderConfig {
            enabled: true,
            source: "// inline".into(),
            ..Default::default()
        });
        assert!(ApplyCard::of(&bundle).screen_shader.is_some());

        bundle.post_shader = Some(rox_core::settings::PostShaderConfig {
            enabled: false,
            name: Some("Dither".into()),
            ..Default::default()
        });
        assert!(ApplyCard::of(&bundle).screen_shader.is_none());
        bundle.post_shader = Some(rox_core::settings::PostShaderConfig {
            enabled: true,
            name: Some("Gone".into()),
            ..Default::default()
        });
        assert!(ApplyCard::of(&bundle).screen_shader.is_none());
        bundle.post_shader = None;
        assert!(ApplyCard::of(&bundle).screen_shader.is_none());
    }

    /// An overwrite from an unsigned live look keeps the card already on disk.
    #[test]
    fn a_save_over_a_workspace_keeps_its_card() {
        let dir = scratch("card");
        let mut first = named_bundle("Nightfall");
        first.meta.author = "Nova".into();
        first.meta.description = "Warm and quiet.".into();
        first.meta.created = "2026-01-02".into();
        first.meta.updated = "2026-01-02".into();
        store_in(&dir, &first, None);

        let again = snapshot_in(&dir, "Nightfall", &Settings::default());
        assert_eq!(again.meta.author, "Nova");
        assert_eq!(again.meta.description, "Warm and quiet.");
        assert_eq!(again.meta.created, "2026-01-02", "the first day survives");
        assert_ne!(again.meta.updated, "2026-01-02", "today stamps updated");

        let mut mine = Settings::default();
        mine.look.bundle.meta.author = "Juniper".into();
        assert_eq!(snapshot_in(&dir, "Nightfall", &mine).meta.author, "Juniper");

        let fresh = snapshot_in(&dir, "Daybreak", &Settings::default());
        assert!(fresh.meta.author.is_empty());
        assert_eq!(fresh.meta.created, fresh.meta.updated);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saved_authors_reads_the_cards_that_name_somebody() {
        let dir = scratch("authors");
        let mut signed = named_bundle("Nightfall");
        signed.meta.author = "Nova".into();
        store_in(&dir, &signed, None);
        store_in(&dir, &named_bundle("Plain"), None);

        let authors = saved_authors_in(&dir);
        assert_eq!(authors.get("Nightfall").map(String::as_str), Some("Nova"));
        assert!(!authors.contains_key("Plain"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn presets_elsewhere_skips_the_live_workspace_and_inlines_pool_names() {
        let dir = scratch("elsewhere");
        let lyrics = |name: &str, shader: serde_json::Value| settings::PanelPreset {
            name: name.into(),
            panel: serde_json::json!({
                "panel_name": "lyrics",
                "children": [],
                "info": { "panel": { "shader": shader } },
            }),
        };

        let mut live = named_bundle("Daylight");
        live.panel_presets
            .push(lyrics("Mine", serde_json::json!({ "enabled": true })));
        store_in(&dir, &live, None);

        let mut other = named_bundle("Nightfall");
        other.shaders.push(NamedShader {
            name: "Grain".into(),
            source: "grain text".into(),
            ..Default::default()
        });
        other.panel_presets.push(lyrics(
            "Glow",
            serde_json::json!({ "enabled": true, "name": "Grain" }),
        ));
        other.panel_presets.push(settings::PanelPreset {
            name: "Scope".into(),
            panel: serde_json::json!({ "panel_name": "spectrum" }),
        });
        store_in(&dir, &other, None);

        let entries = presets_elsewhere_in(&dir, "Daylight", "lyrics");
        assert_eq!(entries.len(), 1, "the live bundle and other kinds stay out");
        assert_eq!(entries[0].scope, PresetScope::Other("Nightfall".into()));
        assert_eq!(entries[0].preset.name, "Glow");

        let shader = &entries[0].preset.panel["info"]["panel"]["shader"];
        assert_eq!(shader["source"], "grain text");
        assert!(shader.get("name").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn relink_takes_only_a_file_that_still_matches() {
        let root = scratch("relink");
        let dir = root.join("Nightfall");
        std::fs::create_dir_all(&dir).unwrap();
        // The hash is over the trimmed text, so an editor's trailing newline
        // still matches.
        std::fs::write(dir.join("Grain.wgsl"), "// grain\n").unwrap();
        std::fs::write(dir.join("Bloom.wgsl"), "// something else").unwrap();

        let mut pool = vec![
            NamedShader {
                name: "Grain".into(),
                source: "// grain".into(),
                path: None,
                assets: Vec::new(),
            },
            NamedShader {
                name: "Bloom".into(),
                source: "// bloom".into(),
                path: None,
                assets: Vec::new(),
            },
            NamedShader {
                name: "Gone".into(),
                source: "// gone".into(),
                path: None,
                assets: Vec::new(),
            },
        ];
        assert!(relink_ejected_in(&root, "Nightfall", &mut pool));
        assert_eq!(pool[0].path, Some(dir.join("Grain.wgsl")));
        assert!(pool[1].path.is_none(), "a drifted file is not the entry");
        assert!(pool[2].path.is_none(), "no file, no bookmark");

        assert!(!relink_ejected_in(&root, "Nightfall", &mut pool[..1]));
        let mut elsewhere = vec![pool[0].clone()];
        elsewhere[0].path = None;
        assert!(!relink_ejected_in(&root, "Daybreak", &mut elsewhere));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unique_name_counts_up_past_collisions() {
        let taken: HashSet<&str> = ["Neon", "Neon (2)"].into_iter().collect();
        assert_eq!(unique_name("Fresh", |c| taken.contains(c)), "Fresh");
        assert_eq!(unique_name("Neon", |c| taken.contains(c)), "Neon (3)");
    }

    /// A name must survive the trip through a filename and back.
    #[test]
    fn file_name_folds_what_a_filename_cant_hold() {
        assert_eq!(file_name("Nightfall"), "Nightfall.roxw");
        assert_eq!(file_name("Drum & Bass / Neuro"), "Drum & Bass   Neuro.roxw");
        assert_eq!(file_name("  padded  "), "padded.roxw");
        assert_eq!(file_name(".hidden"), "hidden.roxw");
        assert_eq!(file_name("..."), "workspace.roxw");
        assert_eq!(file_name(""), "workspace.roxw");
    }

    #[test]
    fn store_lists_and_resolves_by_name() {
        let dir = scratch("store");
        assert!(store_in(&dir, &named_bundle("Nightfall"), None));

        let list = saved_in(&dir);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Nightfall");
        assert!(!list[0].builtin);
        let archive = dir.join("Nightfall.roxw");
        assert_eq!(list[0].path.as_deref(), Some(archive.as_path()));

        let (back, origin) = resolve_in(&dir, "Nightfall").expect("stored bundle resolves");
        assert_eq!(back.name, "Nightfall");
        assert_eq!(back.version, WORKSPACE_VERSION);
        assert_eq!(origin, Origin::Saved(archive.clone()));
        assert!(roxw::read_bundle(&archive).is_ok(), "an archive");

        remove_in(&dir, "Nightfall");
        assert!(saved_in(&dir).is_empty());
        remove_in(&dir, "Nightfall");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saved_lists_roxw_files_by_name() {
        let dir = scratch("list");
        for name in ["Zephyr", "Alpha", "Mid"] {
            store_in(&dir, &named_bundle(name), None);
        }
        std::fs::write(dir.join("notes.txt"), "not a bundle").unwrap();
        std::fs::write(dir.join("Old.json"), "{}").unwrap();
        std::fs::create_dir(dir.join("nested")).unwrap();
        std::fs::create_dir(dir.join(V1_BACKUP)).unwrap();

        let names: Vec<String> = saved_in(&dir).into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["Alpha", "Mid", "Zephyr"]);

        let _ = std::fs::remove_dir_all(&dir);
        assert!(saved_in(&dir).is_empty());
    }

    #[test]
    fn resolve_prefers_saved_over_shipped() {
        let dir = scratch("resolve");
        let shipped_name = shipped()
            .first()
            .map(|e| e.name.clone())
            .expect("a workspace ships");

        let (built_in, origin) = resolve_in(&dir, &shipped_name).expect("shipped resolves");
        assert_eq!(built_in.name, shipped_name);
        assert!(matches!(origin, Origin::Shipped(_)));

        let mut mine = named_bundle(&shipped_name);
        mine.palette_dark.insert("accent".into(), "#336699".into());
        store_in(&dir, &mine, None);
        let (shadowed, _) = resolve_in(&dir, &shipped_name).expect("saved shadows shipped");
        assert_eq!(
            shadowed.palette_dark.get("accent").map(String::as_str),
            Some("#336699")
        );

        assert!(resolve_in(&dir, "does-not-exist").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn all_appends_user_bundles_after_shipped() {
        let list = all();
        let cut = list.iter().position(|e| !e.builtin).unwrap_or(list.len());
        assert!(list[..cut].iter().all(|e| e.builtin));
        assert!(list[cut..].iter().all(|e| !e.builtin));
    }

    #[test]
    fn an_import_names_from_stem_and_dedupes() {
        let dir = scratch("import");
        let path = dir.join("Nightfall.json");
        std::fs::write(&path, serde_json::to_string(&v1_bundle("")).unwrap()).unwrap();

        let empty = scratch("import-empty");
        let bundle = import_file_in(&empty, &path).expect("nameless bundle reads");
        assert_eq!(bundle.name, "Nightfall");
        assert!(empty.join("Nightfall.roxw").is_file());

        let deduped = import_file_in(&empty, &path).expect("bundle reads");
        assert_eq!(deduped.name, "Nightfall (2)");
        assert!(empty.join("Nightfall (2).roxw").is_file());

        // An archive imports the same way and lands as a fresh one.
        let again = import_file_in(&empty, &empty.join("Nightfall.roxw")).expect("archive reads");
        assert_eq!(again.name, "Nightfall (3)");

        std::fs::write(dir.join("notes.txt"), "not a workspace").unwrap();
        assert!(import_file_in(&empty, &dir.join("notes.txt")).is_err());

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn an_import_refuses_a_newer_format() {
        let dir = scratch("newer");
        let mut future = named_bundle("Future");
        future.version = WORKSPACE_VERSION + 1;

        let json = dir.join("Future.json");
        std::fs::write(&json, serde_json::to_string(&future).unwrap()).unwrap();
        assert!(import_file_in(&dir, &json).is_err());

        // Not an archive at all, under the archive's name.
        let archive = dir.join("Future.roxw");
        std::fs::write(&archive, serde_json::to_string(&future).unwrap()).unwrap();
        assert!(import_file_in(&dir, &archive).is_err());

        // The list only reads filenames; resolving is where the refusal bites.
        assert_eq!(saved_in(&dir).len(), 1);
        assert!(resolve_in(&dir, "Future").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What a 1.x export wrote, as far as the version reads.
    fn v1_bundle(name: &str) -> WorkspaceBundle {
        WorkspaceBundle {
            version: 1,
            ..named_bundle(name)
        }
    }

    fn plated_v1(name: &str, bytes: &[u8]) -> serde_json::Value {
        use base64::Engine as _;

        serde_json::json!({
            "version": 1,
            "name": name,
            "shaders": [{
                "name": "Lace",
                "source": "// @asset frame: frame.png",
                "assets": [{
                    "file": "frame.png",
                    "data": base64::engine::general_purpose::STANDARD.encode(bytes),
                }],
            }],
        })
    }

    #[test]
    fn a_v1_json_imports_with_its_images() {
        let dir = scratch("import-v1");
        let into = scratch("import-v1-into");
        let json = dir.join("Lacework.json");
        std::fs::write(&json, plated_v1("Lacework", b"a frame").to_string()).unwrap();

        let bundle = import_file_in(&into, &json).expect("a version 1 export imports");
        assert_eq!(bundle.version, WORKSPACE_VERSION);
        assert_eq!(bundle.assets.len(), 1);
        let asset = &bundle.assets[0];
        assert_eq!(bundle.shaders[0].assets[0].asset, asset.id);

        let archive = into.join("Lacework.roxw");
        let (stored, origin) = resolve_in(&into, "Lacework").expect("stored");
        assert_eq!(origin, Origin::Saved(archive.clone()));
        assert!(stored.shaders[0].assets[0].data.is_empty());
        assert_eq!(
            roxw::read_asset(&archive, &stored, &asset.id).unwrap(),
            b"a frame"
        );

        // Applying it stages the same bytes.
        let staged = into.join("staged");
        stage_in(&staged, &stored, &origin).expect("stages");
        assert_eq!(std::fs::read(staged.join(&asset.file)).unwrap(), b"a frame");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&into);
    }

    #[test]
    fn a_card_edit_keeps_the_files_of_a_workspace_that_isnt_live() {
        let dir = scratch("card-files");
        let json = dir.join("Lacework.json");
        std::fs::write(&json, plated_v1("Lacework", b"a frame").to_string()).unwrap();
        import_file_in(&dir, &json).unwrap();

        let (mut bundle, _) = resolve_in(&dir, "Lacework").unwrap();
        bundle.meta.author = "Nova".into();
        let live = scratch("card-files-live");
        assert!(
            store_in(&dir, &bundle, Some(&live)),
            "the old archive has the file"
        );

        let (back, _) = resolve_in(&dir, "Lacework").unwrap();
        assert_eq!(back.meta.author, "Nova");
        let id = &back.assets[0].id;
        assert_eq!(
            roxw::read_asset(&dir.join("Lacework.roxw"), &back, id).unwrap(),
            b"a frame"
        );

        // A file nobody holds fails the save and leaves the archive alone.
        let mut orphan = back.clone();
        orphan.assets[0].sha256 = "00".repeat(32);
        assert!(!store_in(&dir, &orphan, Some(&live)));
        assert!(resolve_in(&dir, "Lacework").is_some());

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&live);
    }

    #[test]
    fn the_folder_migration_converts_once_and_keeps_the_json() {
        let dir = scratch("migrate-v1");
        let json = dir.join("Lacework.json");
        let text = plated_v1("Lacework", b"a frame").to_string();
        std::fs::write(&json, &text).unwrap();
        std::fs::write(
            dir.join("Plain.json"),
            serde_json::to_string(&v1_bundle("")).unwrap(),
        )
        .unwrap();

        migrate_v1_files_in(&dir);

        let names: Vec<String> = saved_in(&dir).into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["Lacework", "Plain"]);
        assert!(!json.exists());
        assert_eq!(
            std::fs::read_to_string(dir.join(V1_BACKUP).join("Lacework.json")).unwrap(),
            text
        );
        assert!(dir.join(V1_BACKUP).join("Plain.json").is_file());

        let (bundle, _) = resolve_in(&dir, "Lacework").unwrap();
        let id = &bundle.assets[0].id;
        assert_eq!(
            roxw::read_asset(&dir.join("Lacework.roxw"), &bundle, id).unwrap(),
            b"a frame"
        );

        // A second run has nothing to do.
        let before = std::fs::read(dir.join("Lacework.roxw")).unwrap();
        migrate_v1_files_in(&dir);
        assert_eq!(std::fs::read(dir.join("Lacework.roxw")).unwrap(), before);
        assert_eq!(saved_in(&dir).len(), 2);

        // A replay: the archive landed and the JSON never moved.
        std::fs::write(&json, &text).unwrap();
        migrate_v1_files_in(&dir);
        assert_eq!(saved_in(&dir).len(), 2);
        assert!(!json.exists());
        assert!(dir.join(V1_BACKUP).join("Lacework (2).json").is_file());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The skip that makes a replay safe must not drop a different workspace.
    #[test]
    fn a_filename_collision_survives_the_migration() {
        let dir = scratch("migrate-v1-collide");
        let mut theirs = named_bundle("Nightfall");
        theirs
            .palette_dark
            .insert("accent".into(), "#111111".into());
        store_in(&dir, &theirs, None);

        let mut mine = v1_bundle("Nightfall");
        mine.palette_dark.insert("accent".into(), "#222222".into());
        std::fs::write(
            dir.join("Nightfall.json"),
            serde_json::to_string(&mine).unwrap(),
        )
        .unwrap();

        migrate_v1_files_in(&dir);

        let names: Vec<String> = saved_in(&dir).into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["Nightfall", "Nightfall (2)"]);
        let accent = |name: &str| {
            resolve_in(&dir, name)
                .unwrap()
                .0
                .palette_dark
                .get("accent")
                .cloned()
        };
        assert_eq!(accent("Nightfall").as_deref(), Some("#111111"));
        assert_eq!(accent("Nightfall (2)").as_deref(), Some("#222222"));
        assert!(dir.join(V1_BACKUP).join("Nightfall.json").is_file());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn applying_a_workspace_swaps_its_files_in() {
        let dir = scratch("apply-files");
        let json = dir.join("Lacework.json");
        std::fs::write(&json, plated_v1("Lacework", b"a frame").to_string()).unwrap();
        import_file_in(&dir, &json).unwrap();
        let (bundle, origin) = resolve_in(&dir, "Lacework").unwrap();

        // Stands in for the store's swap, which rox-core tests on its own.
        let live = dir.join("assets");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join("old.png"), b"the last look").unwrap();
        let staged = dir.join("assets.incoming");
        let mut table = None;
        swap_in_files_with(&staged, &bundle, &origin, |from, assets| {
            std::fs::remove_dir_all(&live).unwrap();
            std::fs::rename(from, &live).unwrap();
            table = Some(assets);
            Ok(())
        })
        .expect("swaps");

        assert_eq!(table.as_deref(), Some(bundle.assets.as_slice()));
        assert!(!live.join("old.png").exists());
        assert_eq!(
            std::fs::read(live.join(&bundle.assets[0].file)).unwrap(),
            b"a frame"
        );

        // A file that doesn't match its table fails before the swap, and
        // leaves no stage behind.
        let mut broken = bundle.clone();
        broken.assets[0].sha256 = "00".repeat(32);
        let failed = swap_in_files_with(&staged, &broken, &origin, |_, _| {
            panic!("a failed stage never swaps")
        });
        assert!(failed.is_err());
        assert!(!staged.exists());

        // A shipped workspace stages from the files it embeds.
        let (critters, origin) = resolve_in(&dir, "Critters").expect("Critters ships");
        assert_eq!(origin, Origin::Shipped("Critters".into()));
        assert!(!critters.assets.is_empty());
        stage_in(&staged, &critters, &origin).expect("stages");
        for asset in &critters.assets {
            let (bytes, sha256) = asset_store::sha256_file(&staged.join(&asset.file)).unwrap();
            assert_eq!(
                (bytes, sha256.as_str()),
                (asset.bytes, asset.sha256.as_str())
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A crash before the first save must not duplicate every workspace.
    #[test]
    fn migrate_skips_a_name_already_on_disk() {
        let dir = scratch("migrate");
        let mut first = named_bundle("Nightfall");
        first.palette_dark.insert("accent".into(), "#111111".into());
        migrate_saved_in(&dir, first);

        let mut second = named_bundle("Nightfall");
        second
            .palette_dark
            .insert("accent".into(), "#222222".into());
        migrate_saved_in(&dir, second);

        assert_eq!(saved_in(&dir).len(), 1);
        let (kept, _) = resolve_in(&dir, "Nightfall").expect("bundle resolves");
        assert_eq!(
            kept.palette_dark.get("accent").map(String::as_str),
            Some("#111111")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The skip that makes a replay safe must not drop the second one.
    #[test]
    fn migrate_keeps_both_sides_of_a_filename_collision() {
        let dir = scratch("collide");
        let mut first = named_bundle("Live/Studio");
        first.palette_dark.insert("accent".into(), "#111111".into());
        let mut second = named_bundle("Live Studio");
        second
            .palette_dark
            .insert("accent".into(), "#222222".into());
        assert_eq!(file_name(&first.name), file_name(&second.name));

        migrate_saved_in(&dir, first);
        migrate_saved_in(&dir, second);

        let names: Vec<String> = saved_in(&dir).into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["Live Studio", "Live Studio (2)"]);
        assert_eq!(
            resolve_in(&dir, "Live Studio")
                .unwrap()
                .0
                .palette_dark
                .get("accent")
                .map(String::as_str),
            Some("#111111")
        );
        assert_eq!(
            resolve_in(&dir, "Live Studio (2)")
                .unwrap()
                .0
                .palette_dark
                .get("accent")
                .map(String::as_str),
            Some("#222222")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_live_watch_skips_its_own_write() {
        let text = r##"{"bundle":{"palette_dark":{"accent":"#112233"}}}"##;

        assert!(matches!(read_live(text, |_| true), LiveChange::Own(_)));
        match read_live(text, |_| false) {
            LiveChange::Edited(look) => assert_eq!(
                look.bundle.palette_dark.get("accent").map(String::as_str),
                Some("#112233")
            ),
            _ => panic!("an outside edit should re-apply"),
        }
    }

    #[test]
    fn the_live_watch_leaves_a_half_saved_or_newer_file_alone() {
        let half = r##"{"bundle":{"palette_dark":{"accent":"#1122"##;
        assert!(matches!(
            read_live(half, |_| false),
            LiveChange::Unreadable(_)
        ));

        let newer = format!(r#"{{"bundle":{{"version":{}}}}}"#, WORKSPACE_VERSION + 1);
        match read_live(&newer, |_| false) {
            LiveChange::Unreadable(e) => assert!(e.contains("newer"), "{e}"),
            _ => panic!("a newer look should be refused"),
        }
    }

    #[test]
    fn an_edited_table_names_the_files_that_dont_match() {
        let dir = scratch("live-assets");
        std::fs::write(dir.join("plate.png"), b"plate").unwrap();
        std::fs::write(dir.join("moved.png"), b"other bytes").unwrap();

        let entry = |file: &str, bytes: &[u8]| Asset {
            id: file.split('.').next().unwrap().to_string(),
            file: file.to_string(),
            sha256: asset_store::sha256_hex(bytes),
            bytes: bytes.len() as u64,
            ..Asset::default()
        };
        let table = [
            entry("plate.png", b"plate"),
            entry("moved.png", b"moved"),
            entry("gone.png", b"gone"),
            entry("../up.png", b"up"),
        ];

        let problems = asset_problems(&dir, &table);
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert!(problems.iter().all(|p| !p.contains("plate.png")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
