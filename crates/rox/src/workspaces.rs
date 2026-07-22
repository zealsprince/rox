//! Named workspace bundles. Two sources feed one list: the bundles the user
//! saved into settings, and the bundles shipped in the app's assets. A bundle
//! is a whole shareable look - layout presets, the palette, and the
//! appearance - under a name; the settings window lists them and applies one
//! to replace the live look wholesale.
//!
//! A shipped bundle is a [`WorkspaceBundle`] in `assets/workspaces/<name>.json`;
//! its file stem names it when the file carries no name of its own. To ship
//! one: set up a workspace, export it from the settings Workspace page, drop
//! the file in that folder, rebuild.

use std::path::Path;

use gpui::App;

use crate::assets;
use crate::design::palette::{self, Palette};
use crate::settings::{self, Settings, WorkspaceBundle, WORKSPACE_VERSION};

/// A workspace for the settings list: its bundle and whether it ships with
/// the app (read-only) or the user saved it (deletable).
pub struct Entry {
    pub bundle: WorkspaceBundle,
    pub builtin: bool,
}

impl Entry {
    /// The bundle's name, the list's display and lookup key.
    pub fn name(&self) -> &str {
        &self.bundle.name
    }
}

/// The bundles shipped in `assets/workspaces`, named after their files when
/// the file carries no name. A file from a newer format, one that doesn't
/// parse, or one with no usable name is skipped rather than failing the list.
/// Sorted by name for a stable order in the settings window.
fn shipped() -> Vec<Entry> {
    let mut out: Vec<Entry> = assets::shipped_workspaces()
        .into_iter()
        .filter_map(|(stem, bytes)| {
            let mut bundle = serde_json::from_slice::<WorkspaceBundle>(&bytes).ok()?;
            if bundle.version > WORKSPACE_VERSION {
                return None;
            }
            if bundle.name.trim().is_empty() {
                bundle.name = stem;
            }
            (!bundle.name.trim().is_empty()).then_some(Entry {
                bundle,
                builtin: true,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name().cmp(b.name()));
    out
}

/// Every workspace for the settings list: shipped first, then the user's own
/// in save order.
pub fn all(settings: &Settings) -> Vec<Entry> {
    let mut list = shipped();
    list.extend(settings.workspaces.iter().cloned().map(|bundle| Entry {
        bundle,
        builtin: false,
    }));
    list
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
    palette::set(Palette::from_map(&bundle.palette), cx);
    let a = &bundle.appearance;
    palette::set_scalars(a.surface_opacity, a.backdrop_strength, cx);
    settings::set_app_frame(a.frame, cx);
    palette::set_keep_dark(a.keep_dark, cx);
    palette::set_art_theming(a.art_theming, cx);
    settings::set_app_font(a.app_font.clone(), cx);
    settings::set_rating_style(a.rating_style, cx);
    settings::set_hide_menubar(a.hide_menubar, cx);
    settings::set_os_decorations(a.os_decorations);
    crate::workspace::apply_decorations(cx);
}

/// Resolve a workspace name to its bundle, the user's own first so a saved
/// bundle shadows a shipped one of the same name. None when nothing carries
/// that name.
pub fn resolve(settings: &Settings, name: &str) -> Option<WorkspaceBundle> {
    if let Some(saved) = settings.workspaces.iter().find(|w| w.name == name) {
        return Some(saved.clone());
    }
    shipped()
        .into_iter()
        .find(|e| e.name() == name)
        .map(|e| e.bundle)
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
pub fn read_bundle(path: &Path, settings: &Settings) -> Option<WorkspaceBundle> {
    let mut bundle = std::fs::read_to_string(path)
        .ok()
        .and_then(|json| serde_json::from_str::<WorkspaceBundle>(&json).ok())?;
    if bundle.version > WORKSPACE_VERSION {
        return None;
    }
    if bundle.name.trim().is_empty() {
        bundle.name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "imported".to_string());
    }
    bundle.name = unique_name(&bundle.name, |candidate| {
        all(settings).iter().any(|e| e.name() == candidate)
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

    /// A free base name comes back as-is; a taken one gets " (2)", then
    /// " (3)", counting up until it finds an opening. This is how an import
    /// avoids shadowing a workspace already saved.
    #[test]
    fn unique_name_counts_up_past_collisions() {
        let taken: HashSet<&str> = ["Neon", "Neon (2)"].into_iter().collect();
        assert_eq!(unique_name("Fresh", |c| taken.contains(c)), "Fresh");
        assert_eq!(unique_name("Neon", |c| taken.contains(c)), "Neon (3)");
    }

    /// `resolve` prefers the user's saved bundle over a shipped one of the
    /// same name, so a local edit shadows the built-in. An unknown name
    /// resolves to None.
    #[test]
    fn resolve_prefers_saved_over_shipped() {
        let mut s = Settings::default();
        s.workspaces.push(named_bundle("Mine"));
        assert!(resolve(&s, "Mine").is_some());
        assert!(resolve(&s, "does-not-exist").is_none());
    }

    /// `all` lists the shipped bundles first, then the user's own in save
    /// order, and every user bundle is flagged non-builtin.
    #[test]
    fn all_appends_user_bundles_after_shipped() {
        let mut s = Settings::default();
        s.workspaces.push(named_bundle("Alpha"));
        s.workspaces.push(named_bundle("Beta"));
        let list = all(&s);
        // The two user bundles are the last entries, in save order.
        let n = list.len();
        assert!(n >= 2);
        assert_eq!(list[n - 2].name(), "Alpha");
        assert_eq!(list[n - 1].name(), "Beta");
        assert!(!list[n - 2].builtin);
        assert!(!list[n - 1].builtin);
    }

    /// A bundle read from a file with no name of its own takes the file stem,
    /// and a name already in use is deduped so the import never shadows a
    /// saved workspace.
    #[test]
    fn read_bundle_names_from_stem_and_dedupes() {
        let dir = std::env::temp_dir().join(format!("rox-ws-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Nightfall.json");
        // A nameless bundle on disk.
        std::fs::write(&path, serde_json::to_string(&named_bundle("")).unwrap()).unwrap();

        let empty = Settings::default();
        let bundle = read_bundle(&path, &empty).expect("nameless bundle reads");
        assert_eq!(bundle.name, "Nightfall");

        // Same file, but that name is already taken: it dedupes.
        let mut taken = Settings::default();
        taken.workspaces.push(named_bundle("Nightfall"));
        let deduped = read_bundle(&path, &taken).expect("bundle reads");
        assert_eq!(deduped.name, "Nightfall (2)");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A bundle from a newer format version is refused, so an older build
    /// never applies a file it can't understand.
    #[test]
    fn read_bundle_refuses_newer_format() {
        let dir = std::env::temp_dir().join(format!("rox-ws-newer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("future.json");
        let mut future = named_bundle("Future");
        future.version = WORKSPACE_VERSION + 1;
        std::fs::write(&path, serde_json::to_string(&future).unwrap()).unwrap();

        assert!(read_bundle(&path, &Settings::default()).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
