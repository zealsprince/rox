//! Objects: one scene item lifted out of a scene with everything it uses,
//! saved as a `.roxo`, and placed into another scene. A multi-part object
//! is a Group item. The archive's format and its read rules live in
//! [`rox_core::settings::roxo`]; this is the app's half: the installed list,
//! export, place and the place confirm.
//!
//! An object from outside is a stranger's file. Installing only copies it
//! into the objects folder. Placing merges it into the live stores, never
//! replacing anything there, and the shaders it brings arrive unapproved
//! behind the same gate a workspace's do. Nothing it carries switches its
//! own code on. Script associations wait on spike S4.

use std::cell::Cell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{App, AppContext as _, Context, Entity, EntityId, FocusHandle, SharedString, Window};
use gpui_component::input::InputState;
use rox_core::settings::roxo::{self, ObjectBundle};
use rox_core::settings::{
    self, Asset, NamedLayout, NamedShader, WorkspaceBundle, WorkspaceMeta, assets,
};
use rox_dock::{PanelView, StackPanel, TabPanel};
use rox_panel_api::panel::shader;
use rox_panel_api::signal_ui::{self, SignalHost as _};
use rox_scene::{Anchor, Content, SceneItem, SceneState};
use rox_viz::signal::{Route, Signal, Source};

use crate::panels::scene::ScenePanel;
use crate::scene::edit;
use crate::workspace::Workspace;
use crate::workspaces::{self, PendingShader};

thread_local! {
    /// The scene that last held focus, for a document opened from outside.
    static LAST_FOCUSED: Cell<Option<EntityId>> = const { Cell::new(None) };
}

// ── Installed objects ──

/// Copy into `objects/`, name deduped. Places nothing and unpacks nothing.
/// Returns the stored name.
pub fn install(archive: &Path) -> Result<String, String> {
    install_in(&roxo::objects_dir(), archive)
}

fn install_in(dir: &Path, archive: &Path) -> Result<String, String> {
    // Read through rox-package's rules before anything is copied, so a file
    // that fails them never lands in the folder.
    let bundle = roxo::read_bundle(archive)?;

    let base = if bundle.name.trim().is_empty() {
        archive
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        bundle.name.clone()
    };
    let base = settings::safe_file_stem(&base, "object");
    let name = workspaces::unique_name(&base, |candidate| dir.join(file_name(candidate)).exists());

    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let to = dir.join(file_name(&name));
    let tmp = dir.join(format!(".{}.incoming", file_name(&name)));
    let copied = std::fs::copy(archive, &tmp)
        .and_then(|_| std::fs::rename(&tmp, &to))
        .map_err(|e| format!("{}: {e}", to.display()));
    if copied.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    copied?;

    Ok(name)
}

/// Installed objects by name, sorted. Named after the file, so the list is
/// one directory read.
pub fn all() -> Vec<(String, PathBuf)> {
    all_in(&roxo::objects_dir())
}

fn all_in(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut found: Vec<(String, PathBuf)> = entries
        .flatten()
        // A symlink isn't followed: only files rox copied in count.
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|path| is_object(path))
        .filter_map(|path| {
            let stem = path.file_stem()?.to_str()?.to_string();
            (!stem.starts_with('.')).then_some((stem, path))
        })
        .collect();
    found.sort_by_key(|a| a.0.to_lowercase());

    found
}

/// A missing file is a success: the list can be one external delete stale.
pub fn remove(name: &str) -> Result<(), String> {
    let path = path_in(&roxo::objects_dir(), name)?;

    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The file an installed name keeps. A name is only ever joined onto the
/// folder as one plain component.
fn path_in(dir: &Path, name: &str) -> Result<PathBuf, String> {
    if !assets::flat_name(name) || name.starts_with('.') {
        return Err(format!("{name} isn't an object name"));
    }

    Ok(dir.join(file_name(name)))
}

fn file_name(name: &str) -> String {
    format!("{name}.{}", roxo::EXT)
}

fn is_object(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case(roxo::EXT))
}

// ── Export ──

/// What [`export`] writes: the bundle, and each table entry's file.
pub(crate) struct Lifted {
    pub bundle: ObjectBundle,
    pub files: Vec<(Asset, PathBuf)>,
}

/// The card filled in before an export.
pub(crate) struct Card {
    pub name: String,
    pub meta: WorkspaceMeta,
}

/// Lift an item out of a live scene: its tree, the assets it names, the
/// pool shaders and signals it uses, the routes aimed at it.
pub(crate) fn export(
    scene: &ScenePanel,
    item: &str,
    card: Card,
    out: &Path,
    cx: &App,
) -> Result<(), String> {
    // Each grid's dock as it stands now, not as it was loaded.
    let config = scene.saved(cx);

    let mut lifted = lift(
        &config.scene,
        &config.routes,
        item,
        &settings::shader_pool(),
        &scene.hub().pool(),
        &assets::all(),
        &assets::dir(),
    )?;
    if !card.name.trim().is_empty() {
        lifted.bundle.name = card.name.trim().to_string();
    }
    lifted.bundle.meta = card.meta;
    lifted
        .bundle
        .meta
        .stamp(&chrono::Utc::now().format("%Y-%m-%d").to_string());

    roxo::write(out, &lifted.bundle, &lifted.files)
}

/// The pure half of [`export`], against the pools and the store as given.
pub(crate) fn lift(
    scene: &SceneState,
    routes: &[Route],
    id: &str,
    pool: &[NamedShader],
    signals: &[Signal],
    store: &[Asset],
    store_dir: &Path,
) -> Result<Lifted, String> {
    let item = scene
        .find(id)
        .cloned()
        .ok_or_else(|| format!("the scene has no item {id}"))?;

    let mut ids = HashSet::new();
    let mut used = BTreeSet::new();
    walk(&item, &mut |node| {
        ids.insert(node.id.clone());
        if let Some(asset) = node.content.asset() {
            used.insert(asset.to_string());
        }
    });

    let routes: Vec<Route> = routes
        .iter()
        .filter(|route| {
            route
                .target
                .rsplit_once('.')
                .is_some_and(|(id, _)| ids.contains(id))
        })
        .cloned()
        .collect();

    // Exports leak nothing: the shader file bookmarks go.
    let mut json = serde_json::to_value(&item).map_err(|e| format!("{id}: {e}"))?;
    roxo::scrub(&mut json);

    let names = roxo::shader_names(&json);
    let shaders: Vec<NamedShader> = pool
        .iter()
        .filter(|shader| names.contains(&shader.name))
        .map(|shader| NamedShader {
            path: None,
            ..shader.clone()
        })
        .collect();
    used.extend(
        shaders
            .iter()
            .flat_map(|shader| &shader.assets)
            .map(|carried| carried.asset.clone())
            .filter(|asset| !asset.is_empty()),
    );

    // The scene's routes and the item's own, then the signals any of those
    // total, so an aggregate keeps the signal it follows.
    let mut wanted: BTreeSet<u64> = routes.iter().map(|route| route.signal).collect();
    wanted.extend(roxo::route_signals(&json));
    loop {
        let followed: Vec<u64> = signals
            .iter()
            .filter(|signal| wanted.contains(&signal.id))
            .filter_map(|signal| match signal.source {
                Source::Aggregate { of, .. } => Some(of),
                _ => None,
            })
            .filter(|of| !wanted.contains(of))
            .collect();
        if followed.is_empty() {
            break;
        }

        wanted.extend(followed);
    }
    let signals: Vec<Signal> = signals
        .iter()
        .filter(|signal| wanted.contains(&signal.id))
        .cloned()
        .collect();

    let mut table = Vec::new();
    let mut files = Vec::new();
    for asset_id in &used {
        let Some(asset) = store.iter().find(|asset| &asset.id == asset_id) else {
            log::warn!("objects: {id} names asset {asset_id}, which the store doesn't hold");
            continue;
        };
        if !assets::flat_name(&asset.file) {
            continue;
        }

        files.push((asset.clone(), store_dir.join(&asset.file)));
        table.push(asset.clone());
    }

    let bundle = ObjectBundle {
        name: if item.name.trim().is_empty() {
            item.id.clone()
        } else {
            item.name.clone()
        },
        requires: roxo::requires(&json),
        item: json,
        routes,
        signals,
        shaders,
        assets: table,
        ..ObjectBundle::default()
    };

    Ok(Lifted { bundle, files })
}

/// The item and every item inside it, parents first.
fn walk(item: &SceneItem, f: &mut impl FnMut(&SceneItem)) {
    f(item);
    if let Content::Group { items } = &item.content {
        for child in items {
            walk(child, f);
        }
    }
}

// ── Place ──

/// For each object asset whose bytes the store already holds, the store's
/// id. The rest need importing.
pub(crate) fn reused(incoming: &[Asset], store: &[Asset]) -> HashMap<String, String> {
    incoming
        .iter()
        .filter_map(|asset| {
            store
                .iter()
                .find(|held| held.sha256 == asset.sha256 && held.bytes == asset.bytes)
                .map(|held| (asset.id.clone(), held.id.clone()))
        })
        .collect()
}

/// Adds the object's shaders to the pool, merging rather than replacing. A
/// name already held with the same source is reused, one held with other
/// source gets a numbered name. Returns the renames.
pub(crate) fn merge_shaders(
    pool: &mut Vec<NamedShader>,
    incoming: Vec<NamedShader>,
) -> HashMap<String, String> {
    let mut renamed = HashMap::new();

    for shader in incoming {
        if shader.name.trim().is_empty() {
            continue;
        }

        let print = shader::fingerprint(&shader.source);
        let same = |held: &NamedShader| shader::fingerprint(&held.source) == print;

        // The same code placed before under a numbered name is that copy.
        let numbered = format!("{} (", shader.name);
        if let Some(held) = pool.iter().find(|held| {
            (held.name == shader.name || held.name.starts_with(&numbered)) && same(held)
        }) {
            if held.name != shader.name {
                renamed.insert(shader.name.clone(), held.name.clone());
            }
            continue;
        }

        let name = workspaces::unique_name(&shader.name, |candidate| {
            pool.iter().any(|held| held.name == candidate)
        });
        if name != shader.name {
            renamed.insert(shader.name.clone(), name.clone());
        }

        pool.push(NamedShader {
            name,
            path: None,
            ..shader
        });
    }

    renamed
}

/// Adds the object's signals to the pool under fresh ids. A signal id in an
/// object is local, so every one gets the pool's next. A name already taken
/// gets a number. Returns the object's ids mapped to the pool's.
pub(crate) fn merge_signals(pool: &mut Vec<Signal>, incoming: &[Signal]) -> HashMap<u64, u64> {
    let mut next = pool.iter().map(|signal| signal.id).max().unwrap_or(0) + 1;
    let mut ids = HashMap::new();
    for signal in incoming {
        if let std::collections::hash_map::Entry::Vacant(slot) = ids.entry(signal.id) {
            slot.insert(next);
            next += 1;
        }
    }

    let mut added = HashSet::new();
    for signal in incoming {
        if !added.insert(signal.id) {
            continue;
        }

        let mut signal = signal.clone();
        signal.id = ids[&signal.id];
        if let Source::Aggregate { of, .. } = &mut signal.source
            && let Some(&to) = ids.get(of)
        {
            *of = to;
        }
        if !signal.name.is_empty() {
            signal.name = workspaces::unique_name(&signal.name, |candidate| {
                pool.iter().any(|held| held.name == candidate)
            });
        }

        pool.push(signal);
    }

    ids
}

/// The object's item made to fit `scene`: every id fresh, every asset the
/// store's, and its routes re-aimed at the new ids and the pool's signals.
/// A route whose item or signal didn't come along is dropped.
pub(crate) fn rebase(
    mut item: SceneItem,
    routes: &[Route],
    scene: &SceneState,
    assets: &HashMap<String, String>,
    signals: &HashMap<u64, u64>,
) -> (SceneItem, Vec<Route>) {
    let mut taken = HashSet::new();
    scene.walk(&mut |held, _| {
        taken.insert(held.id.clone());
    });

    let mut ids = HashMap::new();
    renumber(&mut item, &mut taken, &mut ids, assets);

    let routes = routes
        .iter()
        .filter_map(|route| {
            let (id, property) = route.target.rsplit_once('.')?;
            let id = ids.get(id)?;
            let signal = *signals.get(&route.signal)?;

            Some(Route {
                target: format!("{id}.{property}"),
                signal,
                ..route.clone()
            })
        })
        .collect();

    (item, routes)
}

/// `SceneState::fresh_id`'s naming, counted against the ids handed out so
/// far in this placement too, since they aren't in the scene yet.
fn renumber(
    item: &mut SceneItem,
    taken: &mut HashSet<String>,
    ids: &mut HashMap<String, String>,
    assets: &HashMap<String, String>,
) {
    let stem = if item.id.is_empty() {
        "item".to_string()
    } else {
        item.id.clone()
    };
    let fresh = if taken.contains(&stem) {
        (2..)
            .map(|n| format!("{stem}-{n}"))
            .find(|id| !taken.contains(id))
            .expect("an unbounded range finds a free id")
    } else {
        stem
    };
    taken.insert(fresh.clone());
    ids.entry(std::mem::replace(&mut item.id, fresh.clone()))
        .or_insert(fresh);

    match &mut item.content {
        Content::Image { asset, .. } => {
            if let Some(to) = assets.get(asset.as_str()) {
                *asset = to.clone();
            }
        }
        Content::Group { items } => {
            for child in items {
                renumber(child, taken, ids, assets);
            }
        }
        _ => {}
    }
}

/// Add an installed object to a scene: assets into the live store, shaders
/// and signals into the pools, ids made fresh, routes re-aimed. One undo
/// step takes the items and their routes back out. What it added to the
/// store and the pools stays, as an image added and deleted again does.
pub(crate) fn place(
    scene: &mut ScenePanel,
    name: &str,
    window: &mut Window,
    cx: &mut Context<ScenePanel>,
) -> Result<String, String> {
    let archive = path_in(&roxo::objects_dir(), name)?;
    let bundle = roxo::read_bundle(&archive)?;

    // Parsed once up front so a broken item stops here, before anything is
    // added to the stores.
    let item: SceneItem =
        serde_json::from_value(bundle.item.clone()).map_err(|e| format!("{name}: {e}"))?;
    let alone = SceneState {
        items: vec![item],
        ..SceneState::default()
    };
    if !edit::fits(&alone, scene.levels_left()) {
        return Err(format!("{name} nests deeper than this scene has room for"));
    }

    // Every file is read and checked against its hash before any goes into
    // the store, so a bad one leaves the store as it was.
    let mut asset_ids = reused(&bundle.assets, &assets::all());
    let mut incoming = Vec::new();
    for asset in &bundle.assets {
        if !asset_ids.contains_key(&asset.id) {
            incoming.push((asset, roxo::read_asset(&archive, asset)?));
        }
    }
    for (asset, bytes) in incoming {
        let stored = assets::import_bytes(&asset.file, &bytes)?;
        asset_ids.insert(asset.id.clone(), stored.id);
    }

    // A shader in the pool runs nowhere until a surface names it, and a
    // stranger's source isn't approved, so it paints as off behind the
    // shared gate until someone approves it there or in the confirm.
    let mut shaders = bundle.shaders.clone();
    for carried in shaders.iter_mut().flat_map(|shader| &mut shader.assets) {
        carried.asset = asset_ids.get(&carried.asset).cloned().unwrap_or_default();
    }
    let mut pool = settings::shader_pool();
    let held = pool.len();
    let renamed = merge_shaders(&mut pool, shaders);
    if pool.len() != held {
        settings::set_shader_pool(pool);
    }

    let hub: Arc<_> = scene.hub().clone();
    let mut signal_ids = HashMap::new();
    if !bundle.signals.is_empty() {
        hub.edit(|pool| signal_ids = merge_signals(pool, &bundle.signals));
        signal_ui::persist_pool_soon(&hub, cx);
    }

    let mut json = bundle.item;
    roxo::rename_shaders(&mut json, &renamed);
    roxo::remap_route_signals(&mut json, &signal_ids);
    let item: SceneItem = serde_json::from_value(json).map_err(|e| format!("{name}: {e}"))?;

    // An item's `action` is a command id and travels as plain data. It only
    // ever reaches `keymap::dispatch`, which runs nothing outside its list.
    let (mut item, routes) = rebase(
        item,
        &bundle.routes,
        &scene.config.scene,
        &asset_ids,
        &signal_ids,
    );
    item.place.anchor = Anchor::Center;
    item.place.position[0] = 0.;
    item.place.position[1] = 0.;

    let id = item.id.clone();
    scene.change_routed(
        move |scene, all| {
            scene.items.push(item);
            all.extend(routes);
        },
        window,
        cx,
    );
    scene.select(Some(id.clone()), window, cx);

    Ok(id)
}

// ── The confirm ──

/// What the place confirm reads out, built once when it opens.
pub(crate) struct PlaceCard {
    /// The installed name, what [`place`] takes.
    pub name: String,
    pub title: String,
    pub byline: Option<SharedString>,
    pub description: Option<SharedString>,
    pub requires: Option<SharedString>,
    /// The unpacked size, said only past [`roxo::WARN_BYTES`].
    pub heavy: Option<SharedString>,
    pub weight: Vec<SharedString>,
    pub shaders: Vec<PendingShader>,
}

impl PlaceCard {
    pub(crate) fn of(name: &str, bundle: &ObjectBundle) -> PlaceCard {
        // The workspace review's own walk, over a bundle holding just the
        // object, so an object's code is listed exactly as a look's is.
        let look = WorkspaceBundle {
            shaders: bundle.shaders.clone(),
            layouts: vec![NamedLayout {
                name: String::new(),
                dump: bundle.item.clone(),
                size: None,
            }],
            ..WorkspaceBundle::default()
        };

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

        let missing: Vec<&str> = bundle
            .requires
            .iter()
            .filter(|id| !rox_services::plugins::is_running(id))
            .map(String::as_str)
            .collect();

        let unpacked = bundle.unpacked();

        PlaceCard {
            name: name.to_string(),
            title: if bundle.name.trim().is_empty() {
                name.to_string()
            } else {
                bundle.name.clone()
            },
            byline: (!byline.is_empty()).then(|| byline.join(", ").into()),
            description: (!meta.description.trim().is_empty())
                .then(|| meta.description.trim().to_string().into()),
            requires: (!missing.is_empty()).then(|| {
                rox_i18n::t!(
                    "workspace-requires-missing",
                    count = missing.len() as u64,
                    plugins = missing.join(", ")
                )
            }),
            heavy: (unpacked > roxo::WARN_BYTES).then(|| {
                // Decimal, since the string says MB.
                let mb = unpacked as f64 / 1_000_000.0;
                rox_i18n::t!("scene-object-heavy", size = format!("{mb:.0}"))
            }),
            weight: crate::scene::cost::warnings(&crate::scene::cost::weigh_object(bundle)),
            shaders: workspaces::unapproved_shaders(&look),
        }
    }

    pub(crate) fn shader_line(&self) -> Option<SharedString> {
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

    /// The same approval the workspace review grants. Only ever called from
    /// the confirm's Approve and Place.
    pub(crate) fn approve_shaders(&self) {
        for pending in &self.shaders {
            shader::approve(&pending.source);
        }
    }
}

/// The dialog a scene shows over itself.
pub(crate) enum Dialog {
    /// The card filled in before an export, then the save prompt.
    Export {
        item: String,
        fields: Vec<Entity<InputState>>,
    },
    /// The one confirm before an object goes in. It holds focus, so Escape
    /// reaches it.
    Place { card: PlaceCard, focus: FocusHandle },
}

/// The export card's fields in order: the name, then the workspace card's
/// author, description and license. Label and placeholder are i18n keys.
pub(crate) const EXPORT_FIELDS: [(&str, &str); 4] = [
    ("scene-object-name", "scene-object-name-placeholder"),
    (
        "settings-workspace-card-author",
        "settings-workspace-card-author-placeholder",
    ),
    (
        "settings-workspace-card-description",
        "scene-object-description-placeholder",
    ),
    (
        "settings-workspace-card-license",
        "settings-workspace-card-license-placeholder",
    ),
];

/// Opens the export card for `item`, its name filled in.
pub(crate) fn open_export(
    scene: &mut ScenePanel,
    item: &str,
    window: &mut Window,
    cx: &mut Context<ScenePanel>,
) {
    let Some(found) = scene.config.scene.find(item) else {
        return;
    };
    let name = if found.name.trim().is_empty() {
        found.id.clone()
    } else {
        found.name.clone()
    };

    let fields: Vec<Entity<InputState>> = EXPORT_FIELDS
        .iter()
        .enumerate()
        .map(|(ix, (_, placeholder))| {
            let seed = if ix == 0 { name.clone() } else { String::new() };
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(rox_i18n::t!(*placeholder))
                    .default_value(seed)
            })
        })
        .collect();
    // Next turn: the button that opened the card takes focus on the same
    // press, after this runs.
    if let Some(first) = fields.first().cloned() {
        cx.defer_in(window, move |_, window, cx| {
            first.update(cx, |input, cx| input.focus(window, cx));
        });
    }

    scene.object_dialog = Some(Dialog::Export {
        item: item.to_string(),
        fields,
    });
    cx.notify();
}

/// The export card's yes: the save prompt, then the write.
pub(crate) fn confirm_export(
    scene: &mut ScenePanel,
    window: &mut Window,
    cx: &mut Context<ScenePanel>,
) {
    let Some(Dialog::Export { item, fields }) = scene.object_dialog.take() else {
        return;
    };
    scene.close_object_dialog(window, cx);

    let typed: Vec<String> = fields
        .iter()
        .map(|input| input.read(cx).value().trim().to_string())
        .collect();
    let card = Card {
        name: typed[0].clone(),
        meta: WorkspaceMeta {
            author: typed[1].clone(),
            description: typed[2].clone(),
            license: typed[3].clone(),
            ..WorkspaceMeta::default()
        },
    };

    let home = dirs::home_dir().unwrap_or_default();
    let file = file_name(&settings::safe_file_stem(&card.name, "object"));
    let rx = cx.prompt_for_new_path(&home, Some(file.as_str()));

    cx.spawn_in(window, async move |this, cx| {
        let Ok(Ok(Some(path))) = rx.await else {
            return;
        };

        this.update_in(cx, |this, window, cx| {
            let label = card.name.clone();
            let toast = match export(this, &item, card, &path, cx) {
                Ok(()) => rox_panel_api::toast::Toast::new(
                    rox_panel_api::panel::Tone::Good,
                    rox_i18n::t!("scene-object-exported", name = label),
                ),
                Err(e) => {
                    log::warn!("objects: exporting {item} to {}: {e}", path.display());
                    rox_panel_api::toast::Toast::new(rox_panel_api::panel::Tone::Bad, e)
                        .title(rox_i18n::t!("scene-object-export-failed", name = label))
                }
            };
            toast.show(window, cx);
        })
        .ok();
    })
    .detach();
}

/// Opens the place confirm for an installed object. A file that no longer
/// reads, a newer one included, says why in a toast instead.
pub(crate) fn offer(
    scene: &mut ScenePanel,
    name: &str,
    window: &mut Window,
    cx: &mut Context<ScenePanel>,
) {
    let bundle = path_in(&roxo::objects_dir(), name).and_then(|path| roxo::read_bundle(&path));
    match bundle {
        Ok(bundle) => {
            let focus = cx.focus_handle();
            focus.focus(window);
            scene.object_dialog = Some(Dialog::Place {
                card: PlaceCard::of(name, &bundle),
                focus,
            });
            cx.notify();
        }
        Err(e) => {
            log::warn!("objects: reading {name}: {e}");
            rox_panel_api::toast::Toast::new(rox_panel_api::panel::Tone::Bad, e)
                .title(rox_i18n::t!("scene-object-place-failed", name = name))
                .show(window, cx);
        }
    }
}

/// Install a file from outside, then offer it to `scene`: a drop, or Add
/// File in the Objects menu.
pub(crate) fn offer_file(
    scene: &mut ScenePanel,
    path: &Path,
    window: &mut Window,
    cx: &mut Context<ScenePanel>,
) {
    match install(path) {
        Ok(name) => offer(scene, &name, window, cx),
        Err(e) => {
            log::warn!("objects: installing {}: {e}", path.display());
            rox_panel_api::toast::Toast::new(rox_panel_api::panel::Tone::Bad, e)
                .title(rox_i18n::t!(
                    "scene-object-add-failed",
                    file = display_name(path)
                ))
                .show(window, cx);
        }
    }
}

/// The confirm's yes. `approve` is Approve and Place, the one click on this
/// path that may write the approved list.
pub(crate) fn confirm_place(
    scene: &mut ScenePanel,
    approve: bool,
    window: &mut Window,
    cx: &mut Context<ScenePanel>,
) {
    let Some(Dialog::Place { card, .. }) = scene.object_dialog.take() else {
        return;
    };
    scene.close_object_dialog(window, cx);

    if approve {
        card.approve_shaders();
    }

    if let Err(e) = place(scene, &card.name, window, cx) {
        log::warn!("objects: placing {}: {e}", card.name);
        rox_panel_api::toast::Toast::new(rox_panel_api::panel::Tone::Bad, e)
            .title(rox_i18n::t!(
                "scene-object-place-failed",
                name = card.title.as_str()
            ))
            .show(window, cx);
    }
}

pub(crate) fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

// ── Which scene ──

/// Called as a scene renders holding focus, itself or inside a grid.
pub(crate) fn note_focused(scene: EntityId) {
    LAST_FOCUSED.set(Some(scene));
}

/// Every scene in the workspace's window, nested ones included: the one that
/// last held focus, or else the first.
pub(crate) fn scene_for(ws: &Workspace, cx: &App) -> Option<Entity<ScenePanel>> {
    fn scenes(node: &Arc<dyn PanelView>, cx: &App, out: &mut Vec<Entity<ScenePanel>>) {
        let view = node.view();
        if let Ok(stack) = view.clone().downcast::<StackPanel>() {
            for child in stack.read(cx).panels() {
                scenes(child, cx, out);
            }
            return;
        }
        if let Ok(tabs) = view.downcast::<TabPanel>() {
            for panel in tabs.read(cx).panels() {
                if let Ok(scene) = panel.view().downcast::<ScenePanel>() {
                    out.push(scene);
                }
            }
        }
    }

    let mut found = Vec::new();
    for dock in ws.docks(cx) {
        scenes(&dock.read(cx).items().view(), cx, &mut found);
    }

    let last = LAST_FOCUSED.get();
    found
        .iter()
        .find(|scene| Some(scene.entity_id()) == last)
        .or(found.first())
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rox_core::settings::{AssetKind, ShaderAsset};
    use serde_json::json;

    fn scene(items: serde_json::Value) -> SceneState {
        serde_json::from_value(json!({ "items": items })).expect("a scene parses")
    }

    fn image(id: &str, asset: &str) -> serde_json::Value {
        json!({ "id": id, "content": { "kind": "image", "asset": asset } })
    }

    fn group(id: &str, items: serde_json::Value) -> serde_json::Value {
        json!({ "id": id, "content": { "kind": "group", "items": items } })
    }

    fn asset(id: &str, bytes: &[u8]) -> Asset {
        Asset {
            id: id.to_string(),
            file: format!("{id}.png"),
            kind: AssetKind::Image,
            sha256: assets::sha256_hex(bytes),
            bytes: bytes.len() as u64,
        }
    }

    fn route(target: &str, signal: u64) -> Route {
        Route {
            signal,
            target: target.to_string(),
            ..Route::default()
        }
    }

    fn signal(id: u64, name: &str) -> Signal {
        Signal {
            id,
            name: name.to_string(),
            ..Signal::default()
        }
    }

    fn shader(name: &str, source: &str) -> NamedShader {
        NamedShader {
            name: name.to_string(),
            source: source.to_string(),
            path: None,
            assets: Vec::new(),
        }
    }

    /// A group of two images and a loose image beside it.
    fn stage() -> SceneState {
        scene(json!([
            group("pair", json!([image("left", "a"), image("right", "b")])),
            image("loose", "c"),
        ]))
    }

    fn lifted(scene: &SceneState, routes: &[Route], signals: &[Signal], id: &str) -> Lifted {
        let store = [asset("a", b"a"), asset("b", b"b"), asset("c", b"c")];
        lift(scene, routes, id, &[], signals, &store, Path::new("/store")).unwrap()
    }

    #[test]
    fn export_brings_the_assets_a_group_uses() {
        let out = lifted(&stage(), &[], &[], "pair");

        let ids: Vec<&str> = out.bundle.assets.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"]);
        assert_eq!(out.files[0].1, Path::new("/store/a.png"));
        assert_eq!(out.bundle.name, "pair");

        // A pool shader the item picks brings its own images along.
        let scene = scene(json!([{
            "id": "glow",
            "content": { "kind": "shader", "shader": { "name": "Glow", "path": "/home/me/g.wgsl" } },
        }]));
        let mut glow = shader("Glow", "fn fs_user() {}");
        glow.path = Some(PathBuf::from("/home/me/g.wgsl"));
        glow.assets = vec![ShaderAsset {
            file: "plate.png".to_string(),
            asset: "c".to_string(),
            data: String::new(),
        }];
        let store = [asset("c", b"c")];
        let out = lift(&scene, &[], "glow", &[glow], &[], &store, Path::new("/s")).unwrap();

        assert_eq!(out.bundle.shaders.len(), 1);
        assert!(out.bundle.shaders[0].path.is_none());
        assert_eq!(out.bundle.assets[0].id, "c");
        assert!(out.bundle.item["content"]["shader"].get("path").is_none());
    }

    #[test]
    fn export_brings_the_routes_aimed_at_a_child() {
        let routes = [
            route("left.opacity", 1),
            route("pair.y", 2),
            route("loose.x", 3),
        ];
        let out = lifted(&stage(), &routes, &[], "pair");

        let targets: Vec<&str> = out
            .bundle
            .routes
            .iter()
            .map(|r| r.target.as_str())
            .collect();
        assert_eq!(targets, ["left.opacity", "pair.y"]);
    }

    #[test]
    fn export_brings_only_the_signals_those_routes_read() {
        let routes = [route("left.opacity", 1), route("loose.x", 3)];
        let total = Signal {
            id: 4,
            source: Source::Aggregate { of: 5, rate: 1. },
            ..Signal::default()
        };
        let signals = [
            signal(1, "Bass"),
            signal(2, "Kick"),
            signal(3, "Level"),
            total,
            signal(5, "Hat"),
        ];

        let out = lifted(&stage(), &routes, &signals, "pair");
        let ids: Vec<u64> = out.bundle.signals.iter().map(|s| s.id).collect();
        assert_eq!(ids, [1]);

        // A shader item's own slot route reads a total, which brings the
        // signal it follows.
        let scene = scene(json!([{
            "id": "glow",
            "content": { "kind": "shader", "shader": { "routes": [{ "signal": 4, "target": "0" }] } },
        }]));
        let out = lifted(&scene, &[], &signals, "glow");
        let ids: Vec<u64> = out.bundle.signals.iter().map(|s| s.id).collect();
        assert_eq!(ids, [4, 5]);
    }

    #[test]
    fn placing_twice_gives_distinct_ids() {
        let object: SceneItem = serde_json::from_value(group(
            "pair",
            json!([image("left", "a"), image("right", "b")]),
        ))
        .unwrap();
        let mut target = stage();

        let (first, _) = rebase(
            object.clone(),
            &[],
            &target,
            &HashMap::new(),
            &HashMap::new(),
        );
        target.items.push(first);
        let (second, _) = rebase(object, &[], &target, &HashMap::new(), &HashMap::new());
        target.items.push(second);

        let mut seen = HashSet::new();
        target.walk(&mut |item, _| {
            assert!(seen.insert(item.id.clone()), "{} twice", item.id);
        });
        assert_eq!(seen.len(), 10);
        assert!(seen.contains("pair-3") && seen.contains("left-3"));
    }

    #[test]
    fn routes_follow_their_items_new_ids() {
        let object: SceneItem = serde_json::from_value(group(
            "pair",
            json!([image("left", "a"), image("right", "b")]),
        ))
        .unwrap();
        let routes = [
            route("pair.y", 7),
            route("left.rz", 8),
            // Its signal didn't come along, so it's dropped.
            route("right.x", 99),
            // Nor did its item.
            route("elsewhere.x", 7),
        ];
        let signals = HashMap::from([(7, 21), (8, 22)]);

        let (item, routes) = rebase(object, &routes, &stage(), &HashMap::new(), &signals);

        assert_eq!(item.id, "pair-2");
        let got: Vec<(String, u64)> = routes.into_iter().map(|r| (r.target, r.signal)).collect();
        assert_eq!(
            got,
            [("pair-2.y".to_string(), 21), ("left-2.rz".to_string(), 22)]
        );
    }

    #[test]
    fn an_asset_already_in_the_store_is_reused() {
        let store = [asset("frame", b"same bytes")];
        let mut carried = asset("plate", b"same bytes");
        carried.file = "plate.png".to_string();
        let other = asset("other", b"other bytes");

        let ids = reused(&[carried, other], &store);
        assert_eq!(ids.get("plate").map(String::as_str), Some("frame"));
        assert!(!ids.contains_key("other"));

        // The item's image follows the store's id.
        let object: SceneItem = serde_json::from_value(image("pic", "plate")).unwrap();
        let (item, _) = rebase(object, &[], &stage(), &ids, &HashMap::new());
        assert_eq!(item.content.asset(), Some("frame"));
    }

    #[test]
    fn a_clashing_shader_name_gets_a_suffix() {
        let mut pool = vec![shader("Glow", "mine"), shader("Plasma", "shared")];

        let renamed = merge_shaders(
            &mut pool,
            vec![shader("Glow", "theirs"), shader("Plasma", "shared")],
        );
        assert_eq!(renamed.get("Glow").map(String::as_str), Some("Glow (2)"));
        assert!(!renamed.contains_key("Plasma"));
        let names: Vec<&str> = pool.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Glow", "Plasma", "Glow (2)"]);

        // The same code placed again finds its numbered copy.
        let again = merge_shaders(&mut pool, vec![shader("Glow", "theirs")]);
        assert_eq!(again.get("Glow").map(String::as_str), Some("Glow (2)"));
        assert_eq!(pool.len(), 3);

        // The item's pick follows the rename.
        let mut item =
            json!({ "id": "s", "content": { "kind": "shader", "shader": { "name": "Glow" } } });
        roxo::rename_shaders(&mut item, &renamed);
        assert_eq!(item["content"]["shader"]["name"], "Glow (2)");
    }

    #[test]
    fn placed_signals_take_fresh_ids_and_names() {
        let mut pool = vec![signal(1, "Bass"), signal(2, "")];
        let total = Signal {
            id: 9,
            name: "Phase".to_string(),
            source: Source::Aggregate { of: 1, rate: 1. },
            ..Signal::default()
        };

        let ids = merge_signals(&mut pool, &[signal(1, "Bass"), total]);

        assert_eq!(ids, HashMap::from([(1, 3), (9, 4)]));
        assert_eq!(pool[2].name, "Bass (2)");
        assert!(matches!(pool[3].source, Source::Aggregate { of: 3, .. }));
    }

    #[test]
    fn install_copies_under_a_deduped_name_and_refuses_what_fails_to_read() {
        let root = std::env::temp_dir().join(format!("rox-objects-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let dir = root.join("objects");

        let src = root.join("pair.roxo");
        let bundle = ObjectBundle {
            name: "Pair".to_string(),
            item: group("pair", json!([])),
            ..ObjectBundle::default()
        };
        roxo::write(&src, &bundle, &[]).unwrap();

        assert_eq!(install_in(&dir, &src).unwrap(), "Pair");
        assert_eq!(install_in(&dir, &src).unwrap(), "Pair (2)");
        let names: Vec<String> = all_in(&dir).into_iter().map(|(name, _)| name).collect();
        assert_eq!(names, ["Pair", "Pair (2)"]);

        std::fs::write(root.join("junk.roxo"), b"not a zip").unwrap();
        assert!(install_in(&dir, &root.join("junk.roxo")).is_err());
        assert_eq!(all_in(&dir).len(), 2);

        assert!(path_in(&dir, "../escape").is_err());
        assert!(path_in(&dir, ".hidden").is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The export without its save prompt, for driving a live run: lifts
    /// `ROX_OBJECT_ITEM` out of the scene in the `workspace.json` at
    /// `ROX_OBJECT_LOOK`, with files from the `assets/` beside it, and writes
    /// `ROX_OBJECT_OUT`.
    #[test]
    #[ignore]
    fn export_from_a_saved_look() {
        let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("set {name}"));
        let look_path = PathBuf::from(var("ROX_OBJECT_LOOK"));
        let id = var("ROX_OBJECT_ITEM");
        let out = PathBuf::from(var("ROX_OBJECT_OUT"));

        let text = std::fs::read_to_string(&look_path).unwrap();
        let look: settings::LookState = serde_json::from_str(&text).unwrap();

        fn configs(value: &serde_json::Value, out: &mut Vec<crate::panels::scene::SceneConfig>) {
            match value {
                serde_json::Value::Object(map) => {
                    if map.get("panel_name").and_then(|name| name.as_str())
                        == Some(crate::panels::scene::PANEL_NAME)
                        && let Some(config) = map.get("info").and_then(|info| info.get("panel"))
                        && let Ok(config) = serde_json::from_value(config.clone())
                    {
                        out.push(config);
                    }
                    map.values().for_each(|child| configs(child, out));
                }
                serde_json::Value::Array(items) => items.iter().for_each(|v| configs(v, out)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        configs(look.layout.as_ref().unwrap(), &mut found);
        let config = found
            .into_iter()
            .find(|config| config.scene.find(&id).is_some())
            .expect("a scene holds the item");

        let store_dir = look_path.parent().unwrap().join("assets");
        let lifted = lift(
            &config.scene,
            &config.routes,
            &id,
            &look.bundle.shaders,
            &look.bundle.signals,
            &look.bundle.assets,
            &store_dir,
        )
        .unwrap();
        roxo::write(&out, &lifted.bundle, &lifted.files).unwrap();
    }
}
