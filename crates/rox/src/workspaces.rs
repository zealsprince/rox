//! Named workspace bundles: a whole shareable look (layout presets, palette,
//! appearance) under a name. Two sources: the user's files under
//! [`settings::workspaces_dir`], and the bundles shipped in the app's assets.
//!
//! A saved workspace is one JSON file, so dropping a shared file in the
//! folder adds it. The list reads names off the filenames and only parses a
//! bundle when its contents are needed, so a menu flyout never parses every
//! workspace per frame.
//!
//! To ship one: export it from the settings Workspace page and drop the file
//! in `assets/workspaces/`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use gpui::{App, SharedString};

use rox_core::settings::{self, NamedShader, Settings, WORKSPACE_VERSION, WorkspaceBundle};
use rox_design::assets;
use rox_design::palette::{self, Palette};
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

/// Refuses a newer format; names a nameless bundle after its file.
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
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
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
    format!("{}.json", settings::safe_file_stem(name, "workspace"))
}

pub fn store(bundle: &WorkspaceBundle) -> bool {
    store_in(&settings::workspaces_dir(), bundle)
}

/// Relative, so it holds when the data dir moves. On another machine it
/// dead-ends and editors skip it.
const SCHEMA_REF: &str = "../schemas/workspace.schema.json";

fn store_in(dir: &Path, bundle: &WorkspaceBundle) -> bool {
    ensure_schema_beside(dir);
    // Stamped during serialization, not via a Value round-trip, so the file
    // keeps its field order with `$schema` first.
    #[derive(serde::Serialize)]
    struct Stamped<'a> {
        #[serde(rename = "$schema")]
        schema: &'a str,
        #[serde(flatten)]
        bundle: &'a WorkspaceBundle,
    }
    let path = dir.join(file_name(&bundle.name));
    note_own_write(&path);
    settings::write_json(
        &path,
        &Stamped {
            schema: SCHEMA_REF,
            bundle,
        },
        "workspace",
    )
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

/// Beside the workspaces folder, not in it: any JSON file in there reads as
/// a workspace.
fn ensure_schema_beside(dir: &Path) {
    let Some(parent) = dir.parent() else {
        return;
    };
    let path = parent.join("schemas").join("workspace.schema.json");
    let text = match serde_json::to_string_pretty(&settings::workspace_schema()) {
        Ok(text) => text + "\n",
        Err(_) => return,
    };
    if std::fs::read_to_string(&path).is_ok_and(|old| old == text) {
        return;
    }
    if let Err(e) = std::fs::create_dir_all(path.parent().expect("schemas dir has a parent")) {
        log::warn!("workspace schema: creating {}: {e}", path.display());
        return;
    }
    if let Err(e) = std::fs::write(&path, text) {
        log::warn!("workspace schema: writing {}: {e}", path.display());
    }
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
    store_in(dir, &bundle);
}

/// Persist the bundle in one write and push each appearance knob live. The
/// layout swap and mini-player roles are the caller's, since they need the
/// workspace whose dock they change.
pub fn apply_look(bundle: &WorkspaceBundle, cx: &mut App) {
    // Persisted up front; the live statics below only repaint.
    let persist = bundle.clone();
    Settings::update(move |s| persist.apply_to(s));
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
    resolve_in(&settings::workspaces_dir(), name)
}

fn resolve_in(dir: &Path, name: &str) -> Option<WorkspaceBundle> {
    read_file(&file_of_in(dir, name)).or_else(|| shipped_bundle(name))
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

/// Deduped against the current workspaces, so an import never shadows one.
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
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
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

/// A file that no longer parses leaves the current look standing.
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
    let parsed = std::fs::read_to_string(&file)
        .map_err(|e| e.to_string())
        .and_then(|text| serde_json::from_str::<WorkspaceBundle>(&text).map_err(|e| e.to_string()));
    match parsed {
        Ok(_) => {
            log::info!("workspace: {active:?} changed on disk, re-applying");
            crate::workspace::apply_workspace_to_front(&active, cx);
        }
        Err(e) => log::warn!(
            "workspace: {} changed on disk but doesn't parse, keeping the current look: {e}",
            file.display()
        ),
    }
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

    /// The splitter and assets only run at registration, so without this a
    /// mistyped `// @pass` or a mangled plate ships as a blank panel. The WGSL
    /// itself is naga's gate.
    #[test]
    fn every_shipped_shader_splits_and_finds_its_images() {
        for (stem, bytes) in assets::shipped_workspaces() {
            let bundle: WorkspaceBundle =
                serde_json::from_slice(&bytes).unwrap_or_else(|err| panic!("{stem}: {err}"));
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
                    let bytes = carried.decode().unwrap_or_else(|err| {
                        panic!("{where_}: {} is not base64: {err}", asset.file)
                    });
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
        let plated = bundle(vec![rox_core::settings::ShaderAsset::from_bytes(
            "plate.png",
            &[1u8, 2, 3],
        )]);
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
        store_in(&dir, &first);

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
        store_in(&dir, &signed);
        store_in(&dir, &named_bundle("Plain"));

        let authors = saved_authors_in(&dir);
        assert_eq!(authors.get("Nightfall").map(String::as_str), Some("Nova"));
        assert!(!authors.contains_key("Plain"));

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
        assert_eq!(file_name("Nightfall"), "Nightfall.json");
        assert_eq!(file_name("Drum & Bass / Neuro"), "Drum & Bass   Neuro.json");
        assert_eq!(file_name("  padded  "), "padded.json");
        assert_eq!(file_name(".hidden"), "hidden.json");
        assert_eq!(file_name("..."), "workspace.json");
        assert_eq!(file_name(""), "workspace.json");
    }

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
        remove_in(&dir, "Nightfall");

        let _ = std::fs::remove_dir_all(&dir);
    }

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

    #[test]
    fn resolve_prefers_saved_over_shipped() {
        let dir = scratch("resolve");
        let shipped_name = shipped()
            .first()
            .map(|e| e.name.clone())
            .expect("a workspace ships");

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

    #[test]
    fn all_appends_user_bundles_after_shipped() {
        let list = all();
        let cut = list.iter().position(|e| !e.builtin).unwrap_or(list.len());
        assert!(list[..cut].iter().all(|e| e.builtin));
        assert!(list[cut..].iter().all(|e| !e.builtin));
    }

    #[test]
    fn read_bundle_names_from_stem_and_dedupes() {
        let dir = scratch("import");
        let path = dir.join("Nightfall.json");
        std::fs::write(&path, serde_json::to_string(&named_bundle("")).unwrap()).unwrap();

        let empty = scratch("import-empty");
        let bundle = read_bundle_in(&empty, &path).expect("nameless bundle reads");
        assert_eq!(bundle.name, "Nightfall");

        store_in(&empty, &named_bundle("Nightfall"));
        let deduped = read_bundle_in(&empty, &path).expect("bundle reads");
        assert_eq!(deduped.name, "Nightfall (2)");

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn read_bundle_refuses_newer_format() {
        let dir = scratch("newer");
        let path = dir.join("Future.json");
        let mut future = named_bundle("Future");
        future.version = WORKSPACE_VERSION + 1;
        std::fs::write(&path, serde_json::to_string(&future).unwrap()).unwrap();

        assert!(read_bundle_in(&dir, &path).is_none());
        // The list only reads filenames; resolving is where the refusal bites.
        assert_eq!(saved_in(&dir).len(), 1);
        assert!(resolve_in(&dir, "Future").is_none());

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
        let kept = resolve_in(&dir, "Nightfall").expect("bundle resolves");
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
