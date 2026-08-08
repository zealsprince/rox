//! Named workspace bundles. Two sources feed one list: the files the user
//! saved under [`settings::workspaces_dir`], and the bundles shipped in the
//! app's assets. A bundle is a whole shareable look (layout presets, the
//! palette, the appearance) under a name; the settings window lists them and
//! applies one to replace the live look wholesale.
//!
//! A saved workspace is one JSON file per bundle, so a saved workspace is
//! already an exported one: drop a shared file in the folder and it joins the
//! list, delete it and it's gone. The list reads names off the filenames and
//! only parses a bundle when something actually needs its contents, which is
//! what keeps a menu flyout from parsing every workspace on every frame.
//!
//! A shipped bundle is a [`WorkspaceBundle`] in `assets/workspaces/<name>.json`;
//! its file stem names it when the file carries no name of its own. To ship
//! one: set up a workspace, export it from the settings Workspace page, drop
//! the file in that folder, rebuild.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use gpui::{App, SharedString};

use rox_core::settings::{self, NamedShader, Settings, WorkspaceBundle, WORKSPACE_VERSION};
use rox_design::assets;
use rox_design::palette::{self, Palette};
use rox_panel_api::panel::shader;

/// A workspace for the settings list: its name, whether it ships with the app
/// (read-only) or the user saved it (deletable), and where to read it from.
/// The bundle itself stays on disk until [`Entry::bundle`] asks for it, so
/// building the list costs a directory read and nothing more.
pub struct Entry {
    /// The bundle's name, the list's display and lookup key.
    pub name: String,
    /// The saved file this entry reads from, None for a shipped bundle.
    pub path: Option<PathBuf>,
    pub builtin: bool,
    /// Who the card says made it, for the lists that credit an author under
    /// the name. Only shipped entries carry it: their bundles are parsed to
    /// build the list anyway, so it costs nothing there, while the saved
    /// list is a directory read and has no bundle in hand. The settings page
    /// fills the saved side in from [`saved_authors`], which reads them once
    /// rather than once a frame.
    pub author: Option<String>,
    /// The asset paths of the preview pictures shipped beside the bundle,
    /// one per theme side (see [`assets::workspace_preview`]), None when
    /// no picture ships or the user saved the bundle. The welcome window's
    /// quick-start tiles draw the side the live theme picks.
    pub preview_dark: Option<SharedString>,
    pub preview_light: Option<SharedString>,
}

/// Read a bundle file, refusing one from a newer format and naming it after
/// its file when it carries no name of its own. The shared reader behind the
/// saved list, an import, and a shipped file.
fn read_file(path: &Path) -> Option<WorkspaceBundle> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut bundle = serde_json::from_str::<WorkspaceBundle>(&text).ok()?;
    if bundle.version > WORKSPACE_VERSION {
        return None;
    }
    if bundle.name.trim().is_empty() {
        bundle.name = stem_of(path)?;
    }
    Some(bundle)
}

/// A path's file stem as a name, None when there isn't a usable one.
fn stem_of(path: &Path) -> Option<String> {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.trim().is_empty())
}

/// A shipped bundle by name, parsed out of the assets on demand.
fn shipped_bundle(name: &str) -> Option<WorkspaceBundle> {
    assets::shipped_workspaces()
        .into_iter()
        .find_map(|(stem, bytes)| {
            let mut bundle = serde_json::from_slice::<WorkspaceBundle>(&bytes).ok()?;
            if bundle.version > WORKSPACE_VERSION {
                return None;
            }
            if bundle.name.trim().is_empty() {
                bundle.name = stem;
            }
            (bundle.name == name).then_some(bundle)
        })
}

/// The bundles shipped in `assets/workspaces`, named after their files when
/// the file carries no name. A file from a newer format, one that doesn't
/// parse, or one with no usable name is skipped rather than failing the list.
/// Sorted by name for a stable order in the settings window and the welcome
/// window's quick-start tiles.
pub fn shipped() -> Vec<Entry> {
    let mut out: Vec<Entry> = assets::shipped_workspaces()
        .into_iter()
        .filter_map(|(stem, bytes)| {
            let bundle = serde_json::from_slice::<WorkspaceBundle>(&bytes).ok()?;
            if bundle.version > WORKSPACE_VERSION {
                return None;
            }
            // The pictures are keyed by the file stem, not the bundle's own
            // name, so look them up before falling back to the stem for one.
            let preview_dark = assets::workspace_preview(&stem, palette::Mode::Dark);
            let preview_light = assets::workspace_preview(&stem, palette::Mode::Light);
            let author = Some(bundle.meta.author.clone()).filter(|a| !a.trim().is_empty());
            let name = match bundle.name.trim() {
                "" => stem,
                named => named.to_string(),
            };
            (!name.trim().is_empty()).then_some(Entry {
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

/// The user's saved workspaces, one per JSON file in [`settings::workspaces_dir`],
/// sorted by name. Named after the file rather than the bundle inside, so the
/// list costs one directory read: a file whose bundle carries a different name
/// is a hand-edit, and the file wins so what you see matches what's on disk.
/// A missing folder is an empty list, the state before the first save.
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
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| {
            Some(Entry {
                name: stem_of(&path)?,
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

/// Who made each saved workspace, by name. The one place the saved list
/// does parse its bundles, so a caller that wants authors pays for them
/// once and holds the answer: the list itself stays a directory read, and
/// a bundle is a page of layout dumps nobody should reparse per frame.
/// Workspaces whose card names nobody drop out.
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

/// The current look as a bundle under a name, ready to save: what
/// [`WorkspaceBundle::from_settings`] snapshots, plus the card of whatever
/// that name already holds.
///
/// A save and an overwrite are the same write here, and an overwrite is a
/// new snapshot of a workspace that already exists, so the card someone
/// filled in on it survives (see
/// [`WorkspaceMeta::carry_forward`](rox_core::settings::WorkspaceMeta::carry_forward)).
/// Only the user's own saved bundles are looked up: saving under a shipped
/// name is a fork, and a fork has no business arriving signed by whoever
/// made the original.
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

/// Trust every shader the build's own workspaces carry, once at startup and
/// before a window can paint one.
///
/// A shader only registers once its hash is approved, and a shipped look's
/// panels would otherwise come up blank asking the user to agree to code
/// that arrived with the binary. Same argument the panel presets make for
/// themselves: installing rox is the agreement. Bundles that don't parse are
/// skipped the way [`shipped`] skips them, since a shipped file that's
/// broken is a build problem and there's nobody to tell about it at startup.
pub fn trust_shipped_shaders() {
    let prints = assets::shipped_workspaces()
        .into_iter()
        .filter_map(|(_, bytes)| serde_json::from_slice::<WorkspaceBundle>(&bytes).ok())
        .filter(|bundle| bundle.version <= WORKSPACE_VERSION)
        .flat_map(|bundle| bundle_fingerprints(&bundle));
    settings::trust_shipped(prints);
}

/// Every shader source a bundle carries, hashed: the pool it travels with,
/// the screen shader it wears, and the ones riding its layout dumps as panel
/// chrome or as a Shader panel's config. Empty sources drop out, since there
/// is nothing there to trust.
fn bundle_fingerprints(bundle: &WorkspaceBundle) -> Vec<String> {
    bundle
        .shaders
        .iter()
        .map(|shader| shader.source.clone())
        .chain(bundle.post_shader.iter().map(|post| post.source.clone()))
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

/// One shader a bundle carries that this machine has never agreed to run.
/// The unit the apply confirm lists and an Approve click walks.
pub struct PendingShader {
    /// What the dialog calls it: the pool entry's name where the source is
    /// one the bundle's author named, and the head of its hash where it only
    /// rides a layout dump and has no name to give.
    pub label: String,
    /// The source itself, which is what an approval is over.
    pub source: String,
}

/// Every distinct shader a bundle carries that this machine hasn't approved,
/// in the order a reader would meet them: the pool first, then the screen
/// shader, then whatever rides the layout dumps.
///
/// Distinct by source, so the same WGSL sitting in the pool and inlined on
/// the panel that wears it counts once, and the pool's pass running first is
/// what gives that one entry its name. Already-approved and builtin sources
/// drop out through [`shader::approved`], along with empty ones: there's
/// nothing there to agree to.
///
/// A panel that names a pool entry still carries whatever source it had
/// inline before the promotion, and the dump walk can't see the name, so a
/// stale inline copy that matches nothing in the pool is listed. That's the
/// safe way round: it's code the bundle is carrying, and the approval it
/// asks for is the one the panel would need if it ever pointed back at it.
pub fn unapproved_shaders(bundle: &WorkspaceBundle) -> Vec<PendingShader> {
    let named = bundle
        .shaders
        .iter()
        .map(|shader| (Some(shader.name.clone()), shader.source.clone()));
    let screen = bundle
        .post_shader
        .iter()
        // A screen shader pointing at a pool entry runs the pool's source,
        // which the pass above already listed. Only an inline one is code of
        // its own.
        .filter(|post| {
            post.name
                .as_deref()
                .is_none_or(|name| !bundle.shaders.iter().any(|shader| shader.name == name))
        })
        .map(|post| (None, post.source.clone()));
    let dumps = bundle
        .layouts
        .iter()
        .flat_map(|layout| settings::dump_shader_sources(&layout.dump))
        .map(|source| (None, source));

    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (name, source) in named.chain(screen).chain(dumps) {
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

/// What a confirm says about the workspace behind it: the name it applies
/// under, the card its author filled in, and the shaders this machine would
/// have to agree to first.
///
/// Built once when the dialog opens rather than read per render. A confirm
/// paints every frame it's up, and what's behind it is a file read and a
/// page of JSON.
pub struct ApplyCard {
    /// The workspace the apply resolves, which is also the dialog's title.
    pub name: String,
    /// Who made it and which version of it this is, as one line. None when
    /// the card says neither.
    pub byline: Option<SharedString>,
    /// The author's own line or two on the look, when they wrote one.
    pub description: Option<SharedString>,
    /// The code riding along that nobody here has agreed to run.
    pub shaders: Vec<PendingShader>,
}

impl ApplyCard {
    /// The card for a named workspace. A name that no longer resolves still
    /// gets a card, bare: the dialog has to render something and the apply
    /// behind it will find the same nothing.
    pub fn for_name(name: &str) -> ApplyCard {
        match resolve(name) {
            Some(bundle) => ApplyCard::of(&bundle),
            None => ApplyCard {
                name: name.to_string(),
                byline: None,
                description: None,
                shaders: Vec::new(),
            },
        }
    }

    /// The card for a bundle in hand, the shape an import has.
    pub fn of(bundle: &WorkspaceBundle) -> ApplyCard {
        let meta = &bundle.meta;
        let mut byline = Vec::new();
        if !meta.author.trim().is_empty() {
            byline.push(format!("by {}", meta.author.trim()));
        }
        if !meta.version.trim().is_empty() {
            byline.push(format!("version {}", meta.version.trim()));
        }
        ApplyCard {
            name: bundle.name.clone(),
            byline: (!byline.is_empty()).then(|| byline.join(", ").into()),
            description: Some(meta.description.trim())
                .filter(|d| !d.is_empty())
                .map(|d| SharedString::from(d.to_string())),
            shaders: unapproved_shaders(bundle),
        }
    }

    /// The line naming what's coming, or None when the bundle brings no code
    /// this machine hasn't already agreed to. Names the pool entries, since
    /// those are what an author talks about their look in; a source that only
    /// rides a dump shows the head of its hash instead.
    pub fn shader_line(&self) -> Option<SharedString> {
        if self.shaders.is_empty() {
            return None;
        }
        let names: Vec<&str> = self
            .shaders
            .iter()
            .map(|shader| shader.label.as_str())
            .collect();
        Some(
            format!(
                "Carries {} shader{}: {}",
                self.shaders.len(),
                if self.shaders.len() == 1 { "" } else { "s" },
                names.join(", ")
            )
            .into(),
        )
    }

    /// Agree to run every shader the bundle brought. The apply side never
    /// calls this on its own: it hangs off the dialog's Approve button, which
    /// is the user saying yes to code that arrived from somewhere else.
    pub fn approve_shaders(&self) {
        for shader in &self.shaders {
            shader::approve(&shader.source);
        }
    }
}

/// Point a freshly applied pool back at the files its shaders were ejected
/// to. A bundle is scrubbed of local bookmarks on the way out, so a look
/// saved and reapplied comes back with every entry unlinked and hot reload
/// dead, even though the working copies are still sitting in the shaders
/// folder. This finds them again.
///
/// The file has to still hold what the entry does, hash for hash. Anything
/// else and the two have drifted apart, and pointing a reload at a file
/// that says something different is how somebody else's WGSL ends up
/// running under a name you trust. Answers whether anything was re-linked,
/// which is what decides if the pool is worth persisting again.
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

/// Every workspace for the settings list: shipped first, then the user's own.
pub fn all() -> Vec<Entry> {
    let mut list = shipped();
    list.extend(saved());
    list
}

/// The file a saved workspace lives in.
pub fn path_for(name: &str) -> PathBuf {
    settings::workspaces_dir().join(file_name(name))
}

/// A name as a filename. The name doubles as the file's, so it goes through
/// the shared sanitizer that every name-as-path in the data directory does;
/// a name of pure punctuation empties out and lands on "workspace".
fn file_name(name: &str) -> String {
    format!("{}.json", settings::safe_file_stem(name, "workspace"))
}

/// Write a bundle to its file, the save and overwrite path both. The bundle's
/// own name picks the file, so a save under a new name lands in a new file and
/// an overwrite lands back on the same one.
pub fn store(bundle: &WorkspaceBundle) -> bool {
    store_in(&settings::workspaces_dir(), bundle)
}

fn store_in(dir: &Path, bundle: &WorkspaceBundle) -> bool {
    settings::write_json(&dir.join(file_name(&bundle.name)), bundle, "workspace")
}

/// Delete a saved workspace's file. A missing file is a success: the list is
/// built from a directory read, so it can be one external delete out of date.
pub fn remove(name: &str) {
    remove_in(&settings::workspaces_dir(), name);
}

fn remove_in(dir: &Path, name: &str) {
    let path = file_of_in(dir, name);
    if let Err(e) = std::fs::remove_file(&path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            log::warn!("workspace: deleting {}: {e}", path.display());
        }
    }
}

/// Write a workspace out of a pre-split settings file. Only the migration
/// calls this, and it has two collisions to tell apart: the same workspace
/// arriving twice, which a crash between the migration and the first save
/// replays, and two different workspaces whose names fold to one filename.
pub(crate) fn migrate_saved(bundle: WorkspaceBundle) {
    migrate_saved_in(&settings::workspaces_dir(), bundle);
}

fn migrate_saved_in(dir: &Path, mut bundle: WorkspaceBundle) {
    let path = dir.join(file_name(&bundle.name));
    match read_file(&path) {
        // This workspace is already out: a replay, so leave the file alone.
        Some(existing) if existing.name == bundle.name => return,
        // A different workspace holds the file its name folds to ("Live/Studio"
        // beside "Live Studio"). It takes a file of its own and the name that
        // matches it, rather than being dropped on the floor.
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
    store_in(dir, &bundle);
}

/// Apply a bundle's whole look to the running app: persist its layouts,
/// palette, and appearance in one write, then push each appearance knob
/// through its live static so every open window repaints. The layout swap
/// and the mini-player roles are the caller's, since those need the
/// workspace whose dock they change; both the settings window's Apply and
/// the empty launcher's workspace tiles go through here for the shared
/// half.
pub fn apply_look(bundle: &WorkspaceBundle, cx: &mut App) {
    // Persist the whole replace up front; the live statics below only
    // repaint, they don't save again.
    let persist = bundle.clone();
    Settings::update(move |s| persist.apply_to(s));
    palette::set_palettes(
        Palette::from_map(&bundle.palette_dark),
        Palette::from_map_over(Palette::light(), &bundle.palette_light),
        cx,
    );
    let a = &bundle.appearance;
    palette::set_scalars(a.surface_opacity, a.backdrop_strength, cx);
    settings::set_app_frame(a.frame, cx);
    settings::set_seams(a.seams, cx);
    palette::set_keep_theme(a.keep_theme, cx);
    palette::set_art_theming(a.art_theming, cx);
    settings::set_app_font(a.app_font.clone(), cx);
    settings::set_rating_style(a.rating_style, cx);
    settings::set_rating_dots(a.rating_dots, cx);
    settings::set_hide_menubar(a.hide_menubar, cx);
    settings::set_os_decorations(a.os_decorations);
    crate::workspace::apply_decorations(cx);
}

/// The file a saved workspace is actually in: the one the list matched, so a
/// hand-dropped file keeps whatever filename it arrived under. Falls back to
/// the name's own file for a workspace that isn't saved yet, which is where a
/// save writes it.
fn file_of_in(dir: &Path, name: &str) -> PathBuf {
    saved_in(dir)
        .into_iter()
        .find(|entry| entry.name == name)
        .and_then(|entry| entry.path)
        .unwrap_or_else(|| dir.join(file_name(name)))
}

/// Resolve a workspace name to its bundle, the user's own first so a saved
/// bundle shadows a shipped one of the same name. None when nothing carries
/// that name, or when the saved file has gone or no longer parses.
pub fn resolve(name: &str) -> Option<WorkspaceBundle> {
    resolve_in(&settings::workspaces_dir(), name)
}

fn resolve_in(dir: &Path, name: &str) -> Option<WorkspaceBundle> {
    read_file(&file_of_in(dir, name)).or_else(|| shipped_bundle(name))
}

/// A name not already taken, appending " (2)", " (3)"... until one is free.
/// How an import names a preset or workspace without shadowing one already
/// saved.
pub(crate) fn unique_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|n| format!("{base} ({n})"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| base.to_string())
}

/// Read a workspace bundle from a shared file, ready to add to the collection:
/// named after the file when the bundle carries no name of its own, and deduped
/// against the current workspaces so an import never shadows one already saved.
/// None when the file isn't a bundle or comes from a newer format.
pub fn read_bundle(path: &Path) -> Option<WorkspaceBundle> {
    read_bundle_in(&settings::workspaces_dir(), path)
}

fn read_bundle_in(dir: &Path, path: &Path) -> Option<WorkspaceBundle> {
    let mut bundle = read_file(path)?;
    if bundle.name.trim().is_empty() {
        bundle.name = "imported".to_string();
    }
    let taken: Vec<String> = shipped()
        .into_iter()
        .chain(saved_in(dir))
        .map(|entry| entry.name)
        .collect();
    bundle.name = unique_name(&bundle.name, |candidate| {
        taken.iter().any(|name| name == candidate)
    });
    Some(bundle)
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

    /// A scratch workspaces folder of this test's own, so a run never reads
    /// or writes the folder the running app keeps its workspaces in.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rox-ws-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Every file in `assets/workspaces` makes it through the shipped
    /// filter. The filter drops a bundle that doesn't parse silently, so
    /// without this a typo in a shipped file just vanishes from the list.
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

    /// The trust pass has to find every shader a bundle carries, wherever it
    /// rides: the pool, the screen shader, panel chrome inside a dump, and a
    /// Shader panel's own config. One missed and that panel comes up blank on
    /// a shipped look.
    #[test]
    fn shipped_trust_collects_every_shader_a_bundle_carries() {
        let bundle = WorkspaceBundle {
            shaders: vec![NamedShader {
                name: "Grain".into(),
                source: "// the pool entry".into(),
                path: None,
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
                        // Nothing to trust in a panel with no shader on it.
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

        // The shipped bundles go through the same collection, so this also
        // says the seeding never panics on what the build actually carries.
        trust_shipped_shaders();
    }

    /// The review a confirm reads out has to find every shader a bundle
    /// carries, count each one once, and leave out the ones this machine has
    /// already agreed to. A miss either way is a dialog that lies: too few
    /// and code arrives unannounced, too many and the count is noise.
    #[test]
    fn the_review_lists_each_unapproved_shader_once() {
        let agreed = "// this one is already agreed to";
        let bundle = WorkspaceBundle {
            shaders: vec![
                NamedShader {
                    name: "Grain".into(),
                    source: "// grain".into(),
                    path: None,
                },
                NamedShader {
                    name: "Bloom".into(),
                    source: "// bloom".into(),
                    path: None,
                },
                NamedShader {
                    name: "Old".into(),
                    source: agreed.into(),
                    path: None,
                },
            ],
            post_shader: Some(rox_core::settings::PostShaderConfig {
                enabled: true,
                name: Some("Grain".into()),
                // A screen shader pointing at the pool runs the pool's entry,
                // so this stale inline copy is not a shader of its own.
                source: "// a stale copy of grain".into(),
                ..Default::default()
            }),
            layouts: vec![rox_core::settings::NamedLayout {
                name: "one".into(),
                size: None,
                dump: serde_json::json!({
                    "panel_name": "StackPanel",
                    "children": [
                        // The pool's own shader, inlined on the panel wearing
                        // it: the same code, so one entry.
                        {
                            "panel_name": "waveform",
                            "info": { "panel": { "shader": { "source": "// grain" }}},
                        },
                        {
                            "panel_name": "shader",
                            "info": { "panel": { "source": "// only in the dump" }},
                        },
                        // Nothing to agree to on a panel with no shader.
                        { "panel_name": "queue", "info": { "panel": { "source": "" }}},
                    ],
                }),
            }],
            ..WorkspaceBundle::default()
        };

        settings::note_approved(&shader::fingerprint(agreed));
        let pending = unapproved_shaders(&bundle);
        let labels: Vec<&str> = pending.iter().map(|s| s.label.as_str()).collect();
        // The pool comes first, so its entries carry the names their author
        // gave them; the dump-only source has none and shows its hash.
        let hashed = shader::fingerprint("// only in the dump")[..8].to_string();
        assert_eq!(labels, ["Grain", "Bloom", hashed.as_str()], "{labels:?}");

        let line = ApplyCard::of(&bundle).shader_line().expect("a shader line");
        assert!(
            line.starts_with("Carries 3 shaders: Grain, Bloom, "),
            "{line}"
        );

        // Agreeing to them is what empties the review, so the same bundle
        // applied twice only asks once.
        for shader in &pending {
            settings::note_approved(&shader::fingerprint(&shader.source));
        }
        assert!(unapproved_shaders(&bundle).is_empty());
        assert!(ApplyCard::of(&bundle).shader_line().is_none());

        for source in ["// grain", "// bloom", "// only in the dump", agreed] {
            settings::forget_approved(&shader::fingerprint(source));
        }
        // A look with no code in it asks nothing, which is what keeps the
        // plain apply exactly the confirm it always was.
        assert!(unapproved_shaders(&WorkspaceBundle::default()).is_empty());
    }

    /// The card a bundle carries is what the confirm reads out: who made it
    /// and what they say it is, folded into a byline the dialog can print.
    #[test]
    fn a_card_reads_out_as_a_byline() {
        let mut bundle = named_bundle("Nightfall");
        bundle.meta.author = "Nova".into();
        bundle.meta.version = "2.1".into();
        bundle.meta.description = "Warm and quiet.".into();
        let card = ApplyCard::of(&bundle);
        assert_eq!(card.name, "Nightfall");
        assert_eq!(
            card.byline.as_ref().map(|line| line.as_ref()),
            Some("by Nova, version 2.1")
        );
        assert_eq!(
            card.description.as_ref().map(|line| line.as_ref()),
            Some("Warm and quiet.")
        );

        // A look nobody signed says nothing rather than printing an empty
        // line where the byline goes.
        let plain = ApplyCard::of(&named_bundle("Plain"));
        assert!(plain.byline.is_none());
        assert!(plain.description.is_none());
    }

    /// Saving over a workspace keeps the card the file already carried, so an
    /// overwrite from a live look that was never signed doesn't wipe what
    /// somebody typed in. A name nobody has saved yet takes the fresh card as
    /// it comes.
    #[test]
    fn a_save_over_a_workspace_keeps_its_card() {
        let dir = scratch("card");
        let mut first = named_bundle("Nightfall");
        first.meta.author = "Nova".into();
        first.meta.description = "Warm and quiet.".into();
        first.meta.created = "2026-01-02".into();
        first.meta.updated = "2026-01-02".into();
        store_in(&dir, &first);

        let again = snapshot_in(&dir, "Nightfall", &Settings::default());
        assert_eq!(again.meta.author, "Nova");
        assert_eq!(again.meta.description, "Warm and quiet.");
        assert_eq!(again.meta.created, "2026-01-02", "the first day survives");
        assert_ne!(again.meta.updated, "2026-01-02", "today stamps updated");

        // A live look carrying its own card signs the save itself.
        let mut mine = Settings::default();
        mine.look.bundle.meta.author = "Juniper".into();
        assert_eq!(snapshot_in(&dir, "Nightfall", &mine).meta.author, "Juniper");

        // Nothing saved under that name yet: the fresh card stands alone.
        let fresh = snapshot_in(&dir, "Daybreak", &Settings::default());
        assert!(fresh.meta.author.is_empty());
        assert_eq!(fresh.meta.created, fresh.meta.updated);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The authors read is the one place the saved list parses its bundles.
    /// Workspaces nobody signed stay out of it, so a row only credits an
    /// author when there is one.
    #[test]
    fn saved_authors_reads_the_cards_that_name_somebody() {
        let dir = scratch("authors");
        let mut signed = named_bundle("Nightfall");
        signed.meta.author = "Nova".into();
        store_in(&dir, &signed);
        store_in(&dir, &named_bundle("Plain"));

        let authors = saved_authors_in(&dir);
        assert_eq!(authors.get("Nightfall").map(String::as_str), Some("Nova"));
        assert!(!authors.contains_key("Plain"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A pool entry re-links to its ejected file only when the file still
    /// says what the entry does. A drifted file, a missing one, and an entry
    /// that already has a bookmark are all left alone: a reload aimed at
    /// text nobody approved is exactly what the gate exists to stop.
    #[test]
    fn relink_takes_only_a_file_that_still_matches() {
        let root = scratch("relink");
        let dir = root.join("Nightfall");
        std::fs::create_dir_all(&dir).unwrap();
        // The eject writes with a trailing newline an editor would add
        // anyway; the hash is over the trimmed text, so it still matches.
        std::fs::write(dir.join("Grain.wgsl"), "// grain\n").unwrap();
        std::fs::write(dir.join("Bloom.wgsl"), "// something else").unwrap();

        let mut pool = vec![
            NamedShader {
                name: "Grain".into(),
                source: "// grain".into(),
                path: None,
            },
            NamedShader {
                name: "Bloom".into(),
                source: "// bloom".into(),
                path: None,
            },
            NamedShader {
                name: "Gone".into(),
                source: "// gone".into(),
                path: None,
            },
        ];
        assert!(relink_ejected_in(&root, "Nightfall", &mut pool));
        assert_eq!(pool[0].path, Some(dir.join("Grain.wgsl")));
        assert!(pool[1].path.is_none(), "a drifted file is not the entry");
        assert!(pool[2].path.is_none(), "no file, no bookmark");

        // Nothing left to link is not news, so the pool doesn't get written
        // out again for it.
        assert!(!relink_ejected_in(&root, "Nightfall", &mut pool[..1]));
        // Another workspace's folder holds none of this look's shaders.
        let mut elsewhere = vec![pool[0].clone()];
        elsewhere[0].path = None;
        assert!(!relink_ejected_in(&root, "Daybreak", &mut elsewhere));

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A free base name comes back as-is; a taken one gets " (2)", then
    /// " (3)", counting up until it finds an opening. This is how an import
    /// avoids shadowing a workspace already saved.
    #[test]
    fn unique_name_counts_up_past_collisions() {
        let taken: HashSet<&str> = ["Neon", "Neon (2)"].into_iter().collect();
        assert_eq!(unique_name("Fresh", |c| taken.contains(c)), "Fresh");
        assert_eq!(unique_name("Neon", |c| taken.contains(c)), "Neon (3)");
    }

    /// A name has to survive the trip through a filename and back, or a
    /// workspace saves to one file and resolves from another. The characters
    /// a filename can't hold fold to spaces, and a name of pure punctuation
    /// still lands somewhere rather than on a dotfile or an empty name.
    #[test]
    fn file_name_folds_what_a_filename_cant_hold() {
        assert_eq!(file_name("Nightfall"), "Nightfall.json");
        assert_eq!(file_name("Drum & Bass / Neuro"), "Drum & Bass   Neuro.json");
        assert_eq!(file_name("  padded  "), "padded.json");
        // A leading dot would hide the file; a name of nothing else lands on
        // the fallback rather than writing ".json".
        assert_eq!(file_name(".hidden"), "hidden.json");
        assert_eq!(file_name("..."), "workspace.json");
        assert_eq!(file_name(""), "workspace.json");
    }

    /// A stored bundle comes back by name: the save picks the file, the list
    /// reads it off the folder, and resolve parses it. The round trip the
    /// whole file-backed collection rests on.
    #[test]
    fn store_lists_and_resolves_by_name() {
        let dir = scratch("store");
        assert!(store_in(&dir, &named_bundle("Nightfall")));

        let list = saved_in(&dir);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Nightfall");
        assert!(!list[0].builtin);

        let back = resolve_in(&dir, "Nightfall").expect("stored bundle resolves");
        assert_eq!(back.name, "Nightfall");

        remove_in(&dir, "Nightfall");
        assert!(saved_in(&dir).is_empty());
        // Deleting one that's already gone is a no-op, not a panic: the list
        // can be one external delete out of date.
        remove_in(&dir, "Nightfall");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The folder is the list: files that aren't bundles are ignored, the
    /// order is by name whatever order the filesystem hands them back in, and
    /// a folder that doesn't exist yet is an empty list rather than an error.
    #[test]
    fn saved_lists_json_files_by_name() {
        let dir = scratch("list");
        for name in ["Zephyr", "Alpha", "Mid"] {
            store_in(&dir, &named_bundle(name));
        }
        std::fs::write(dir.join("notes.txt"), "not a bundle").unwrap();
        std::fs::create_dir(dir.join("nested")).unwrap();

        let names: Vec<String> = saved_in(&dir).into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["Alpha", "Mid", "Zephyr"]);

        let _ = std::fs::remove_dir_all(&dir);
        assert!(saved_in(&dir).is_empty());
    }

    /// A saved bundle shadows a shipped one of the same name, so a local edit
    /// wins over the built-in. An unknown name resolves to None.
    #[test]
    fn resolve_prefers_saved_over_shipped() {
        let dir = scratch("resolve");
        let shipped_name = shipped()
            .first()
            .map(|e| e.name.clone())
            .expect("a workspace ships");

        // The shipped one resolves out of the assets while nothing shadows it.
        let built_in = resolve_in(&dir, &shipped_name).expect("shipped resolves");
        assert_eq!(built_in.name, shipped_name);

        let mut mine = named_bundle(&shipped_name);
        mine.palette_dark.insert("accent".into(), "#336699".into());
        store_in(&dir, &mine);
        let shadowed = resolve_in(&dir, &shipped_name).expect("saved shadows shipped");
        assert_eq!(
            shadowed.palette_dark.get("accent").map(String::as_str),
            Some("#336699")
        );

        assert!(resolve_in(&dir, "does-not-exist").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `all` lists the shipped bundles first, then the user's own, and every
    /// saved one is flagged non-builtin.
    #[test]
    fn all_appends_user_bundles_after_shipped() {
        let list = all();
        let cut = list.iter().position(|e| !e.builtin).unwrap_or(list.len());
        assert!(list[..cut].iter().all(|e| e.builtin));
        assert!(list[cut..].iter().all(|e| !e.builtin));
    }

    /// A bundle read from a file with no name of its own takes the file stem,
    /// and a name already in use is deduped so the import never shadows a
    /// saved workspace.
    #[test]
    fn read_bundle_names_from_stem_and_dedupes() {
        let dir = scratch("import");
        let path = dir.join("Nightfall.json");
        // A nameless bundle on disk, the shape a hand-written file arrives in.
        std::fs::write(&path, serde_json::to_string(&named_bundle("")).unwrap()).unwrap();

        let empty = scratch("import-empty");
        let bundle = read_bundle_in(&empty, &path).expect("nameless bundle reads");
        assert_eq!(bundle.name, "Nightfall");

        // Same file, but that name is already saved: it dedupes.
        store_in(&empty, &named_bundle("Nightfall"));
        let deduped = read_bundle_in(&empty, &path).expect("bundle reads");
        assert_eq!(deduped.name, "Nightfall (2)");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }

    /// A bundle from a newer format version is refused, so an older build
    /// never applies a file it can't understand. Refused everywhere it can
    /// arrive: an import, and a file dropped straight in the folder.
    #[test]
    fn read_bundle_refuses_newer_format() {
        let dir = scratch("newer");
        let path = dir.join("Future.json");
        let mut future = named_bundle("Future");
        future.version = WORKSPACE_VERSION + 1;
        std::fs::write(&path, serde_json::to_string(&future).unwrap()).unwrap();

        assert!(read_bundle_in(&dir, &path).is_none());
        // The folder still lists it, since the list only reads filenames;
        // resolving is where the refusal bites.
        assert_eq!(saved_in(&dir).len(), 1);
        assert!(resolve_in(&dir, "Future").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The migration writes a workspace out once and leaves an existing file
    /// alone, so a crash before the first save can't duplicate every
    /// workspace on the next launch.
    #[test]
    fn migrate_skips_a_name_already_on_disk() {
        let dir = scratch("migrate");
        let mut first = named_bundle("Nightfall");
        first.palette_dark.insert("accent".into(), "#111111".into());
        migrate_saved_in(&dir, first);

        // The same name coming round a second time leaves the file alone.
        let mut second = named_bundle("Nightfall");
        second
            .palette_dark
            .insert("accent".into(), "#222222".into());
        migrate_saved_in(&dir, second);

        assert_eq!(saved_in(&dir).len(), 1);
        let kept = resolve_in(&dir, "Nightfall").expect("bundle resolves");
        assert_eq!(
            kept.palette_dark.get("accent").map(String::as_str),
            Some("#111111")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two workspaces whose names fold to one filename both survive the
    /// migration. The skip that makes a replay safe would otherwise drop the
    /// second one silently, which is a workspace gone on upgrade.
    #[test]
    fn migrate_keeps_both_sides_of_a_filename_collision() {
        let dir = scratch("collide");
        let mut first = named_bundle("Live/Studio");
        first.palette_dark.insert("accent".into(), "#111111".into());
        let mut second = named_bundle("Live Studio");
        second
            .palette_dark
            .insert("accent".into(), "#222222".into());
        // Both fold to the same file, so the second has to land elsewhere.
        assert_eq!(file_name(&first.name), file_name(&second.name));

        migrate_saved_in(&dir, first);
        migrate_saved_in(&dir, second);

        let names: Vec<String> = saved_in(&dir).into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["Live Studio", "Live Studio (2)"]);
        // The renamed one keeps its own look, and the first is untouched.
        assert_eq!(
            resolve_in(&dir, "Live Studio")
                .unwrap()
                .palette_dark
                .get("accent")
                .map(String::as_str),
            Some("#111111")
        );
        assert_eq!(
            resolve_in(&dir, "Live Studio (2)")
                .unwrap()
                .palette_dark
                .get("accent")
                .map(String::as_str),
            Some("#222222")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
